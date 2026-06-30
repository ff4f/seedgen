use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use sqlx::postgres::{PgArguments, PgRow};
use sqlx::{PgPool, Postgres, Row};

use crate::generators::{create_generator, Generator, Value};
use crate::introspection::{
    introspect, Column, DataType, ForeignKey, IntrospectionError, SchemaGraph, Table,
};
use crate::resolver::{resolve, DeferredUpdate, ResolverError};
use crate::scenario::parser::{ColumnOverride, CountExpression, ScenarioConfig};
use crate::semantic::{
    detect_generator, ConstraintHandler, ConstraintHandlerKind, GeneratorType, ValidationResult,
};

#[derive(Debug, thiserror::Error)]
pub enum GenerateError {
    #[error("introspection failed: {0}")]
    Introspection(#[from] IntrospectionError),

    #[error("resolver failed: {0}")]
    Resolver(#[from] ResolverError),

    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("constraint violation in `{table}.{column}`: {reason}")]
    ConstraintViolation {
        table: String,
        column: String,
        reason: String,
    },

    #[error("exhausted retries generating unique value for `{table}.{column}`")]
    UniqueExhausted { table: String, column: String },

    #[error("no parent rows generated for FK `{table}.{column}` -> `{parent}`")]
    MissingParent {
        table: String,
        column: String,
        parent: String,
    },

    #[error("output mode {0} not yet implemented")]
    UnsupportedOutput(&'static str),
}

#[derive(Debug, Clone)]
pub struct GenerateConfig {
    pub seed: u64,
    pub rows_per_table: usize,
    pub scenario: Option<ScenarioConfig>,
    pub output_mode: OutputMode,
    pub include_tables: Option<Vec<String>>,
    pub exclude_tables: Option<Vec<String>>,
    pub truncate_first: bool,
}

impl Default for GenerateConfig {
    fn default() -> Self {
        Self {
            seed: 42,
            rows_per_table: 10,
            scenario: None,
            output_mode: OutputMode::DirectInsert,
            include_tables: None,
            exclude_tables: None,
            truncate_first: false,
        }
    }
}

#[derive(Debug, Clone)]
pub enum OutputMode {
    DirectInsert,
    SqlFile(PathBuf),
    Json(PathBuf),
    Stdout,
}

#[derive(Debug, Clone)]
pub struct GenerationResult {
    pub tables_seeded: Vec<TableResult>,
    pub total_rows: usize,
    pub duration: Duration,
    pub seed_used: u64,
}

#[derive(Debug, Clone)]
pub struct TableResult {
    pub name: String,
    pub rows_inserted: usize,
    pub duration: Duration,
}

const BATCH_SIZE: usize = 1000;
const MAX_UNIQUE_RETRIES: usize = 32;

pub async fn generate(
    pool: &PgPool,
    config: &GenerateConfig,
) -> Result<GenerationResult, GenerateError> {
    let started = Instant::now();

    let schema = introspect(pool).await?;
    let plan = resolve(&schema)?;

    // Lifecycle mode: if the scenario declared a `lifecycle:` block, hand off to
    // the lifecycle engine. Otherwise fall through to the standard flow below.
    if let Some(lifecycle) = config.scenario.as_ref().and_then(|s| s.lifecycle.clone()) {
        let table_lifecycles = config
            .scenario
            .as_ref()
            .map(|s| s.table_lifecycles.clone())
            .unwrap_or_default();

        if config.truncate_first {
            let targets: Vec<String> = plan
                .ordered_tables
                .iter()
                .filter(|t| table_lifecycles.contains_key(*t) && should_include_table(t, config))
                .cloned()
                .collect();
            if !targets.is_empty() {
                truncate_tables(pool, &targets).await?;
            }
        }

        let engine = crate::lifecycle::LifecycleEngine::new(lifecycle, table_lifecycles);
        return engine.execute(&schema, &plan, config, pool).await;
    }

    let selected: Vec<String> = plan
        .ordered_tables
        .iter()
        .filter(|t| should_include_table(t, config))
        .cloned()
        .collect();

    if config.truncate_first && !selected.is_empty() {
        truncate_tables(pool, &selected).await?;
    }

    let mut rng = ChaCha8Rng::seed_from_u64(config.seed);
    let mut generated_ids: HashMap<String, Vec<Value>> = HashMap::new();
    let mut unique_states: HashMap<String, UniqueState> = HashMap::new();
    let mut table_results = Vec::with_capacity(selected.len());
    let mut total_rows = 0usize;

    let mut sql_file: Option<std::fs::File> = match &config.output_mode {
        OutputMode::SqlFile(path) => Some(std::fs::File::create(path)?),
        _ => None,
    };

    let empty_overrides: HashMap<String, ColumnOverride> = HashMap::new();

    for table_name in &selected {
        let table = schema
            .table(table_name)
            .expect("table from plan must exist in schema");

        let row_count = resolve_row_count(table_name, config, &generated_ids);
        let table_started = Instant::now();

        let overrides = config
            .scenario
            .as_ref()
            .and_then(|s| s.tables.get(table_name))
            .map(|ts| &ts.overrides)
            .unwrap_or(&empty_overrides);

        let outcome = generate_table(
            pool,
            &schema,
            table,
            row_count,
            &plan.deferred_updates,
            &mut rng,
            &mut generated_ids,
            &mut unique_states,
            overrides,
            &config.output_mode,
        )
        .await?;

        if let Some(sql) = outcome.sql {
            match &config.output_mode {
                OutputMode::SqlFile(_) => {
                    if let Some(f) = sql_file.as_mut() {
                        use std::io::Write;
                        f.write_all(sql.as_bytes())?;
                    }
                }
                OutputMode::Stdout => {
                    print!("{sql}");
                }
                _ => {}
            }
        }

        table_results.push(TableResult {
            name: table_name.clone(),
            rows_inserted: outcome.inserted,
            duration: table_started.elapsed(),
        });
        total_rows += outcome.inserted;
    }

    if !plan.deferred_updates.is_empty() && matches!(config.output_mode, OutputMode::DirectInsert) {
        run_deferred_updates(pool, &plan.deferred_updates, &generated_ids, &mut rng).await?;
    }

    Ok(GenerationResult {
        tables_seeded: table_results,
        total_rows,
        duration: started.elapsed(),
        seed_used: config.seed,
    })
}

fn should_include_table(name: &str, config: &GenerateConfig) -> bool {
    if let Some(include) = &config.include_tables {
        if !include.iter().any(|t| t == name) {
            return false;
        }
    }
    if let Some(exclude) = &config.exclude_tables {
        if exclude.iter().any(|t| t == name) {
            return false;
        }
    }
    true
}

fn resolve_row_count(
    table: &str,
    config: &GenerateConfig,
    generated_ids: &HashMap<String, Vec<Value>>,
) -> usize {
    let Some(scenario) = config.scenario.as_ref() else {
        return config.rows_per_table;
    };
    let Some(ts) = scenario.tables.get(table) else {
        return config.rows_per_table;
    };
    evaluate_count(&ts.count, config, generated_ids)
}

fn evaluate_count(
    count: &CountExpression,
    config: &GenerateConfig,
    generated_ids: &HashMap<String, Vec<Value>>,
) -> usize {
    match count {
        CountExpression::Fixed(n) => *n,
        CountExpression::PerParent {
            parent_table,
            min,
            max,
        } => {
            let parent_count = generated_ids
                .get(parent_table)
                .map(|v| v.len())
                .unwrap_or(0);
            if parent_count == 0 {
                return config.rows_per_table;
            }
            let avg = (*min + *max) as f64 / 2.0;
            (parent_count as f64 * avg).round() as usize
        }
        CountExpression::PercentageOf { table, percentage } => {
            let base = generated_ids.get(table).map(|v| v.len()).unwrap_or(0);
            (base as f64 * percentage / 100.0).round() as usize
        }
    }
}

async fn truncate_tables(pool: &PgPool, tables: &[String]) -> Result<(), GenerateError> {
    let quoted: Vec<String> = tables.iter().map(|t| quote_ident(t)).collect();
    let sql = format!(
        "TRUNCATE TABLE {} RESTART IDENTITY CASCADE",
        quoted.join(", ")
    );
    sqlx::raw_sql(&sql).execute(pool).await?;
    Ok(())
}

/// Generate and emit one table's rows. Used by both the standard `generate`
/// flow and the lifecycle engine (which calls it once per bucket).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn generate_table(
    pool: &PgPool,
    schema: &SchemaGraph,
    table: &Table,
    row_count: usize,
    deferred: &[DeferredUpdate],
    rng: &mut ChaCha8Rng,
    generated_ids: &mut HashMap<String, Vec<Value>>,
    // Per-table UNIQUE tracking, keyed by table name. The standard flow passes a
    // fresh map (one call per table); the lifecycle engine reuses the same map
    // across buckets so uniqueness holds over the whole table, not just one
    // bucket. See `init_unique_state`.
    unique_states: &mut HashMap<String, UniqueState>,
    // Per-column value overrides (scenario `distribution`/`range`, or
    // profile-derived). Empty for callers that don't apply overrides.
    overrides: &HashMap<String, ColumnOverride>,
    output: &OutputMode,
) -> Result<GenerateOutcome, GenerateError> {
    if row_count == 0 {
        generated_ids.entry(table.name.clone()).or_default();
        return Ok(GenerateOutcome {
            inserted: 0,
            sql: None,
        });
    }

    let materialize_auto_ids = !matches!(output, OutputMode::DirectInsert);
    let plan = build_column_plan(schema, table, deferred, materialize_auto_ids, overrides);
    if plan.included.is_empty() {
        return Err(GenerateError::ConstraintViolation {
            table: table.name.clone(),
            column: "*".into(),
            reason: "no insertable columns".into(),
        });
    }

    let unique_state = unique_states
        .entry(table.name.clone())
        .or_insert_with(|| init_unique_state(table, &plan));

    let mut rows: Vec<Vec<Value>> = Vec::with_capacity(row_count);
    for i in 0..row_count {
        let row = generate_row(table, &plan, unique_state, generated_ids, i, rng)?;
        rows.push(row);
    }

    match output {
        OutputMode::DirectInsert => {
            let mut inserted = 0;
            for batch in rows.chunks(BATCH_SIZE) {
                let returned = insert_batch(pool, table, &plan, batch).await?;
                store_generated_ids(table, &plan, batch, &returned, generated_ids);
                inserted += batch.len();
            }
            Ok(GenerateOutcome {
                inserted,
                sql: None,
            })
        }
        OutputMode::SqlFile(_) | OutputMode::Stdout => {
            let col_names: Vec<String> = plan
                .included
                .iter()
                .map(|c| c.column.name.clone())
                .collect();
            let sql = crate::output::generate_sql(&table.name, &col_names, &rows);
            store_synthetic_ids_from_rows(table, &plan, &rows, generated_ids);
            Ok(GenerateOutcome {
                inserted: rows.len(),
                sql: Some(sql),
            })
        }
        OutputMode::Json(_) => Err(GenerateError::UnsupportedOutput("Json")),
    }
}

pub(crate) struct GenerateOutcome {
    pub(crate) inserted: usize,
    pub(crate) sql: Option<String>,
}

struct ColumnPlan<'a> {
    /// Columns we actually insert values for (in order).
    included: Vec<IncludedColumn<'a>>,
    /// Columns omitted from INSERT but expected back via RETURNING (identity / nextval).
    auto_assigned: Vec<&'a Column>,
}

struct IncludedColumn<'a> {
    column: &'a Column,
    source: ColumnSource,
}

enum ColumnSource {
    Fk {
        parent_table: String,
    },
    DeferredFk,
    Generator(Box<dyn Generator>),
    SyntheticId,
    /// Weighted categorical pick from a scenario/profile `distribution` override.
    Distribution(WeightedValues),
    /// Bounded numeric value from a scenario/profile `range` override.
    RangeOverride {
        min: f64,
        max: f64,
    },
}

/// A deterministic weighted sampler over pre-typed values. Entries are stored in
/// a fixed (sorted) order so sampling is reproducible regardless of the source
/// map's iteration order.
struct WeightedValues {
    values: Vec<Value>,
    cumulative: Vec<f64>,
    total: f64,
}

impl WeightedValues {
    fn new(pairs: Vec<(Value, f64)>) -> Self {
        let mut values = Vec::with_capacity(pairs.len());
        let mut cumulative = Vec::with_capacity(pairs.len());
        let mut total = 0.0;
        for (value, weight) in pairs {
            total += weight.max(0.0);
            values.push(value);
            cumulative.push(total);
        }
        Self {
            values,
            cumulative,
            total,
        }
    }

    fn pick(&self, rng: &mut ChaCha8Rng) -> Value {
        if self.values.is_empty() || self.total <= 0.0 {
            return Value::Null;
        }
        let r = rng.gen_range(0.0..self.total);
        for (i, &c) in self.cumulative.iter().enumerate() {
            if r < c {
                return self.values[i].clone();
            }
        }
        self.values[self.values.len() - 1].clone()
    }
}

fn build_column_plan<'a>(
    schema: &'a SchemaGraph,
    table: &'a Table,
    deferred: &[DeferredUpdate],
    materialize_auto_ids: bool,
    overrides: &HashMap<String, ColumnOverride>,
) -> ColumnPlan<'a> {
    let mut included = Vec::new();
    let mut auto_assigned = Vec::new();

    for column in &table.columns {
        if column.is_generated {
            continue;
        }
        if is_auto_assigned(column) {
            if materialize_auto_ids {
                included.push(IncludedColumn {
                    column,
                    source: ColumnSource::SyntheticId,
                });
            } else {
                auto_assigned.push(column);
            }
            continue;
        }
        if is_deferred(deferred, &table.name, &column.name) {
            included.push(IncludedColumn {
                column,
                source: ColumnSource::DeferredFk,
            });
            continue;
        }
        if let Some(fk) = find_fk(schema, &table.name, &column.name) {
            included.push(IncludedColumn {
                column,
                source: ColumnSource::Fk {
                    parent_table: fk.to_table.clone(),
                },
            });
            continue;
        }

        // A scenario/profile override on this column's values takes precedence
        // over the inferred generator (FK integrity above still wins).
        if let Some(source) = overrides
            .get(&column.name)
            .and_then(|ov| override_source(ov, column))
        {
            included.push(IncludedColumn { column, source });
            continue;
        }

        let gen_type = detect_generator(column, &schema.enums);
        if matches!(gen_type, GeneratorType::Skip) {
            continue;
        }
        included.push(IncludedColumn {
            column,
            source: ColumnSource::Generator(create_generator(&gen_type)),
        });
    }

    ColumnPlan {
        included,
        auto_assigned,
    }
}

fn is_auto_assigned(col: &Column) -> bool {
    col.is_identity
        || col
            .default_value
            .as_deref()
            .map(|d| d.contains("nextval"))
            .unwrap_or(false)
}

fn is_deferred(deferred: &[DeferredUpdate], table: &str, column: &str) -> bool {
    deferred
        .iter()
        .any(|d| d.table == table && d.column == column)
}

fn find_fk<'a>(schema: &'a SchemaGraph, table: &str, column: &str) -> Option<&'a ForeignKey> {
    schema
        .foreign_keys
        .iter()
        .find(|fk| fk.from_table == table && fk.from_column == column)
}

/// Build a value source from a column override, if it is a kind the generator
/// applies directly here. Returns `None` for override kinds handled elsewhere.
fn override_source(ov: &ColumnOverride, column: &Column) -> Option<ColumnSource> {
    match ov {
        ColumnOverride::Distribution(dist) => {
            // Sort by key so sampling order is deterministic (the map is unordered).
            let mut pairs: Vec<(&String, &f64)> = dist.iter().collect();
            pairs.sort_by(|a, b| a.0.cmp(b.0));
            let values = pairs
                .into_iter()
                .map(|(k, w)| (key_to_value(k, &column.data_type), *w))
                .collect();
            Some(ColumnSource::Distribution(WeightedValues::new(values)))
        }
        ColumnOverride::Range { min, max } => Some(ColumnSource::RangeOverride {
            min: *min,
            max: *max,
        }),
        _ => None,
    }
}

/// Convert a distribution key string into a typed [`Value`] for `data_type`.
fn key_to_value(key: &str, data_type: &DataType) -> Value {
    match data_type {
        DataType::SmallInt | DataType::Integer | DataType::BigInt => key
            .parse::<i64>()
            .map(Value::Int)
            .unwrap_or_else(|_| Value::String(key.to_string())),
        DataType::Boolean => key
            .parse::<bool>()
            .map(Value::Bool)
            .unwrap_or_else(|_| Value::String(key.to_string())),
        DataType::Real | DataType::DoublePrecision | DataType::Numeric => key
            .parse::<f64>()
            .map(Value::Float)
            .unwrap_or_else(|_| Value::String(key.to_string())),
        _ => Value::String(key.to_string()),
    }
}

/// Generate a bounded numeric value for a `range` override.
fn range_value(data_type: &DataType, min: f64, max: f64, rng: &mut ChaCha8Rng) -> Value {
    let (lo, hi) = if min <= max { (min, max) } else { (max, min) };
    match data_type {
        DataType::SmallInt | DataType::Integer | DataType::BigInt => {
            let lo_i = lo.round() as i64;
            let hi_i = hi.round() as i64;
            if lo_i >= hi_i {
                Value::Int(lo_i)
            } else {
                Value::Int(rng.gen_range(lo_i..=hi_i))
            }
        }
        _ => {
            if lo >= hi {
                Value::Float(lo)
            } else {
                Value::Float(rng.gen_range(lo..hi))
            }
        }
    }
}

pub(crate) struct UniqueState {
    /// Indexed by column position within `ColumnPlan::included`.
    handlers: HashMap<usize, ConstraintHandler>,
}

fn init_unique_state(table: &Table, plan: &ColumnPlan<'_>) -> UniqueState {
    let unique_cols: HashSet<&str> = table
        .constraints
        .iter()
        .filter(|c| {
            matches!(c.kind, crate::introspection::ConstraintKind::Unique) && c.columns.len() == 1
        })
        .map(|c| c.columns[0].as_str())
        .collect();

    let mut handlers = HashMap::new();
    for (idx, inc) in plan.included.iter().enumerate() {
        if unique_cols.contains(inc.column.name.as_str()) {
            handlers.insert(
                idx,
                ConstraintHandler::new(ConstraintHandlerKind::Unique {
                    seen: HashSet::new(),
                }),
            );
        }
    }
    UniqueState { handlers }
}

fn generate_row(
    table: &Table,
    plan: &ColumnPlan<'_>,
    unique: &mut UniqueState,
    generated_ids: &HashMap<String, Vec<Value>>,
    row_index: usize,
    rng: &mut ChaCha8Rng,
) -> Result<Vec<Value>, GenerateError> {
    let mut row = Vec::with_capacity(plan.included.len());

    for (idx, inc) in plan.included.iter().enumerate() {
        let value = match &inc.source {
            ColumnSource::SyntheticId => Value::Int((row_index + 1) as i64),
            ColumnSource::Fk { parent_table } => {
                let parents = generated_ids.get(parent_table).cloned().unwrap_or_default();
                if parents.is_empty() {
                    if inc.column.is_nullable {
                        Value::Null
                    } else {
                        return Err(GenerateError::MissingParent {
                            table: table.name.clone(),
                            column: inc.column.name.clone(),
                            parent: parent_table.clone(),
                        });
                    }
                } else {
                    parents[rng.gen_range(0..parents.len())].clone()
                }
            }
            ColumnSource::DeferredFk => Value::Null,
            ColumnSource::Generator(g) => {
                let mut value = g.generate(rng);
                if let Some(handler) = unique.handlers.get_mut(&idx) {
                    let mut attempts = 0;
                    loop {
                        match handler.validate(&inc.column.name, &value) {
                            ValidationResult::Valid => break,
                            ValidationResult::Retry => {
                                attempts += 1;
                                if attempts >= MAX_UNIQUE_RETRIES {
                                    return Err(GenerateError::UniqueExhausted {
                                        table: table.name.clone(),
                                        column: inc.column.name.clone(),
                                    });
                                }
                                value = g.generate(rng);
                            }
                            ValidationResult::Invalid(reason) => {
                                return Err(GenerateError::ConstraintViolation {
                                    table: table.name.clone(),
                                    column: inc.column.name.clone(),
                                    reason,
                                });
                            }
                        }
                    }
                }
                value
            }
            ColumnSource::Distribution(weighted) => weighted.pick(rng),
            ColumnSource::RangeOverride { min, max } => {
                range_value(&inc.column.data_type, *min, *max, rng)
            }
        };
        row.push(value);
    }

    Ok(row)
}

async fn insert_batch(
    pool: &PgPool,
    table: &Table,
    plan: &ColumnPlan<'_>,
    rows: &[Vec<Value>],
) -> Result<Vec<PgRow>, GenerateError> {
    let col_count = plan.included.len();
    let col_list = plan
        .included
        .iter()
        .map(|c| quote_ident(&c.column.name))
        .collect::<Vec<_>>()
        .join(", ");

    let placeholders = (0..rows.len())
        .map(|row_idx| {
            let parts = (0..col_count)
                .map(|c| format!("${}", row_idx * col_count + c + 1))
                .collect::<Vec<_>>()
                .join(", ");
            format!("({parts})")
        })
        .collect::<Vec<_>>()
        .join(", ");

    let returning = if plan.auto_assigned.is_empty() {
        String::new()
    } else {
        let cols = plan
            .auto_assigned
            .iter()
            .map(|c| quote_ident(&c.name))
            .collect::<Vec<_>>()
            .join(", ");
        format!(" RETURNING {cols}")
    };

    let sql = format!(
        "INSERT INTO {} ({}) VALUES {}{}",
        quote_ident(&table.name),
        col_list,
        placeholders,
        returning
    );

    let mut query = sqlx::query(&sql);
    for row in rows {
        for value in row {
            query = bind_value(query, value);
        }
    }

    if plan.auto_assigned.is_empty() {
        query.execute(pool).await?;
        Ok(Vec::new())
    } else {
        Ok(query.fetch_all(pool).await?)
    }
}

fn store_generated_ids(
    table: &Table,
    plan: &ColumnPlan<'_>,
    rows: &[Vec<Value>],
    returned: &[PgRow],
    generated_ids: &mut HashMap<String, Vec<Value>>,
) {
    let bucket = generated_ids.entry(table.name.clone()).or_default();

    let id_col_opt: Option<&Column> = plan
        .auto_assigned
        .iter()
        .find(|c| c.name == "id")
        .copied()
        .or_else(|| plan.auto_assigned.first().copied());

    if let Some(id_col) = id_col_opt {
        for row in returned {
            if let Ok(v) = read_value_from_row(row, id_col) {
                bucket.push(v);
            }
        }
    } else {
        // No auto-assigned column — find an "id"-like column in `included` (a column we generated).
        let id_idx = plan
            .included
            .iter()
            .position(|c| c.column.name == "id")
            .or_else(|| {
                plan.included
                    .iter()
                    .position(|c| matches!(c.column.data_type, DataType::Uuid))
            });

        if let Some(idx) = id_idx {
            for row in rows {
                if let Some(v) = row.get(idx) {
                    bucket.push(v.clone());
                }
            }
        }
    }
}

fn store_synthetic_ids_from_rows(
    table: &Table,
    plan: &ColumnPlan<'_>,
    rows: &[Vec<Value>],
    generated_ids: &mut HashMap<String, Vec<Value>>,
) {
    let bucket = generated_ids.entry(table.name.clone()).or_default();
    let id_idx = plan
        .included
        .iter()
        .position(|c| matches!(c.source, ColumnSource::SyntheticId) && c.column.name == "id")
        .or_else(|| {
            plan.included
                .iter()
                .position(|c| matches!(c.source, ColumnSource::SyntheticId))
        });
    if let Some(idx) = id_idx {
        for row in rows {
            if let Some(v) = row.get(idx) {
                bucket.push(v.clone());
            }
        }
    }
}

async fn run_deferred_updates(
    pool: &PgPool,
    deferred: &[DeferredUpdate],
    generated_ids: &HashMap<String, Vec<Value>>,
    rng: &mut ChaCha8Rng,
) -> Result<(), GenerateError> {
    for upd in deferred {
        let parents = match generated_ids.get(&upd.references_table) {
            Some(v) if !v.is_empty() => v,
            _ => continue,
        };
        let children = match generated_ids.get(&upd.table) {
            Some(v) if !v.is_empty() => v,
            _ => continue,
        };

        for child_id in children {
            let parent_id = &parents[rng.gen_range(0..parents.len())];
            let sql = format!(
                "UPDATE {} SET {} = $1 WHERE id = $2",
                quote_ident(&upd.table),
                quote_ident(&upd.column),
            );
            let query = sqlx::query(&sql);
            let query = bind_value(query, parent_id);
            let query = bind_value(query, child_id);
            query.execute(pool).await?;
        }
    }
    Ok(())
}

fn bind_value<'q>(
    q: sqlx::query::Query<'q, Postgres, PgArguments>,
    value: &Value,
) -> sqlx::query::Query<'q, Postgres, PgArguments> {
    match value {
        Value::String(s) => q.bind(s.clone()),
        Value::Int(i) => q.bind(*i),
        Value::Float(f) => q.bind(*f),
        Value::Bool(b) => q.bind(*b),
        Value::Null => q.bind(Option::<String>::None),
        Value::Uuid(s) => q.bind(sqlx::types::Uuid::parse_str(s).expect("valid uuid")),
        Value::Timestamp(dt) => q.bind(*dt),
        Value::Date(d) => q.bind(*d),
        Value::Json(j) => q.bind(j.clone()),
    }
}

fn read_value_from_row(row: &PgRow, col: &Column) -> Result<Value, sqlx::Error> {
    let name = col.name.as_str();
    Ok(match col.data_type {
        DataType::SmallInt => Value::Int(row.try_get::<i16, _>(name)? as i64),
        DataType::Integer => Value::Int(row.try_get::<i32, _>(name)? as i64),
        DataType::BigInt => Value::Int(row.try_get::<i64, _>(name)?),
        DataType::Real => Value::Float(row.try_get::<f32, _>(name)? as f64),
        DataType::DoublePrecision => Value::Float(row.try_get::<f64, _>(name)?),
        DataType::Boolean => Value::Bool(row.try_get(name)?),
        DataType::Text | DataType::Varchar | DataType::Char => Value::String(row.try_get(name)?),
        DataType::Uuid => {
            let u: sqlx::types::Uuid = row.try_get(name)?;
            Value::Uuid(u.to_string())
        }
        DataType::Timestamp | DataType::TimestampTz => Value::Timestamp(row.try_get(name)?),
        DataType::Date => Value::Date(row.try_get(name)?),
        _ => Value::Null,
    })
}

fn quote_ident(name: &str) -> String {
    let escaped = name.replace('"', "\"\"");
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let c = GenerateConfig::default();
        assert_eq!(c.seed, 42);
        assert_eq!(c.rows_per_table, 10);
        assert!(matches!(c.output_mode, OutputMode::DirectInsert));
    }

    #[test]
    fn test_should_include_table_no_filters() {
        let c = GenerateConfig::default();
        assert!(should_include_table("anything", &c));
    }

    #[test]
    fn test_should_include_table_include_list() {
        let c = GenerateConfig {
            include_tables: Some(vec!["users".into(), "posts".into()]),
            ..GenerateConfig::default()
        };
        assert!(should_include_table("users", &c));
        assert!(!should_include_table("comments", &c));
    }

    #[test]
    fn test_should_include_table_exclude_list() {
        let c = GenerateConfig {
            exclude_tables: Some(vec!["audit_log".into()]),
            ..GenerateConfig::default()
        };
        assert!(should_include_table("users", &c));
        assert!(!should_include_table("audit_log", &c));
    }

    #[test]
    fn test_resolve_row_count_default() {
        let c = GenerateConfig {
            rows_per_table: 25,
            ..GenerateConfig::default()
        };
        let empty = HashMap::new();
        assert_eq!(resolve_row_count("users", &c, &empty), 25);
    }

    #[test]
    fn test_resolve_row_count_scenario_fixed_override() {
        use crate::scenario::parser::{ScenarioConfig, TableScenario};
        let mut tables = HashMap::new();
        tables.insert(
            "users".into(),
            TableScenario {
                count: CountExpression::Fixed(100),
                overrides: HashMap::new(),
            },
        );
        let c = GenerateConfig {
            rows_per_table: 10,
            scenario: Some(ScenarioConfig {
                seed: None,
                tables,
                ..Default::default()
            }),
            ..GenerateConfig::default()
        };
        let empty = HashMap::new();
        assert_eq!(resolve_row_count("users", &c, &empty), 100);
        assert_eq!(resolve_row_count("posts", &c, &empty), 10);
    }

    #[test]
    fn test_resolve_row_count_per_parent_uses_average() {
        use crate::scenario::parser::{ScenarioConfig, TableScenario};
        let mut tables = HashMap::new();
        tables.insert(
            "orders".into(),
            TableScenario {
                count: CountExpression::PerParent {
                    parent_table: "users".into(),
                    min: 2,
                    max: 8,
                },
                overrides: HashMap::new(),
            },
        );
        let c = GenerateConfig {
            rows_per_table: 1,
            scenario: Some(ScenarioConfig {
                seed: None,
                tables,
                ..Default::default()
            }),
            ..GenerateConfig::default()
        };
        // 10 users * avg(2,8) = 10 * 5 = 50 orders
        let mut ids = HashMap::new();
        ids.insert("users".into(), (0..10).map(Value::Int).collect());
        assert_eq!(resolve_row_count("orders", &c, &ids), 50);
    }

    #[test]
    fn test_resolve_row_count_percentage_of() {
        use crate::scenario::parser::{ScenarioConfig, TableScenario};
        let mut tables = HashMap::new();
        tables.insert(
            "follows".into(),
            TableScenario {
                count: CountExpression::PercentageOf {
                    table: "users".into(),
                    percentage: 30.0,
                },
                overrides: HashMap::new(),
            },
        );
        let c = GenerateConfig {
            rows_per_table: 1,
            scenario: Some(ScenarioConfig {
                seed: None,
                tables,
                ..Default::default()
            }),
            ..GenerateConfig::default()
        };
        let mut ids = HashMap::new();
        ids.insert("users".into(), (0..100).map(Value::Int).collect());
        assert_eq!(resolve_row_count("follows", &c, &ids), 30);
    }

    #[test]
    fn test_quote_ident_basic() {
        assert_eq!(quote_ident("users"), "\"users\"");
    }

    #[test]
    fn test_quote_ident_escapes_internal_quotes() {
        assert_eq!(quote_ident("we\"ird"), "\"we\"\"ird\"");
    }

    #[test]
    fn test_is_auto_assigned_identity() {
        let col = Column {
            name: "id".into(),
            data_type: DataType::Integer,
            is_nullable: false,
            is_identity: true,
            is_generated: false,
            default_value: None,
            max_length: None,
            numeric_precision: None,
            numeric_scale: None,
        };
        assert!(is_auto_assigned(&col));
    }

    #[test]
    fn test_is_auto_assigned_serial_nextval() {
        let col = Column {
            name: "id".into(),
            data_type: DataType::Integer,
            is_nullable: false,
            is_identity: false,
            is_generated: false,
            default_value: Some("nextval('users_id_seq'::regclass)".into()),
            max_length: None,
            numeric_precision: None,
            numeric_scale: None,
        };
        assert!(is_auto_assigned(&col));
    }

    #[test]
    fn test_is_auto_assigned_regular_default() {
        let col = Column {
            name: "created_at".into(),
            data_type: DataType::Timestamp,
            is_nullable: false,
            is_identity: false,
            is_generated: false,
            default_value: Some("now()".into()),
            max_length: None,
            numeric_precision: None,
            numeric_scale: None,
        };
        assert!(!is_auto_assigned(&col));
    }
}
