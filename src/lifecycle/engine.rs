//! Lifecycle engine — the orchestrator.
//!
//! The engine adds exactly two things on top of the existing generation core:
//! (1) how many rows each table gets per time bucket, and (2) the per-bucket
//! timestamp/distribution constraints. It does NOT re-implement generation,
//! semantics, or output — for the actual row work it calls the existing
//! [`crate::generate::generate_table`] once per (bucket, table). See
//! `LIFECYCLE.md` → "engine.rs".

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::Write;
use std::time::{Duration, Instant};

use chrono::{Datelike, NaiveDate, NaiveDateTime};
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use sqlx::{PgPool, Row};

use crate::generate::{
    generate_table, GenerateConfig, GenerateError, GenerationResult, OutputMode, TableResult,
    UniqueState,
};
use crate::generators::Value;
use crate::introspection::SchemaGraph;
use crate::output::quote_ident;
use crate::resolver::InsertionPlan;
use crate::scenario::parser::{ColumnOverride, CountExpression, ScenarioConfig, TableScenario};

use crate::lifecycle::churn::ChurnModel;
use crate::lifecycle::config::{BucketGranularity, LifecycleConfig, TimeBucket};
use crate::lifecycle::growth::GrowthModel;
use crate::lifecycle::pool::EntityPool;
use crate::lifecycle::seasonality::SeasonalityModel;
use crate::lifecycle::temporal::TemporalConstraint;
use crate::lifecycle::timeline::TimelineDistribution;

/// The column whose values are constrained to the bucket window for root
/// (non-`follows`) tables. Child tables time themselves via temporal
/// constraints relative to their parents instead.
const BUCKET_TIME_COLUMN: &str = "created_at";

/// Convention column: if a churning table has a nullable timestamp column with
/// this name, the engine records each entity's churn time there (a soft-delete
/// stamp alongside the configured churn column). Active rows keep it NULL.
const CHURN_TIME_COLUMN: &str = "churned_at";

/// Per-table lifecycle behavior, assembled by the YAML parser and consumed by
/// the engine.
#[derive(Debug, Clone, PartialEq)]
pub struct TableLifecycle {
    pub growth: GrowthModel,
    pub churn: Option<ChurnModel>,
    pub seasonality: Option<SeasonalityModel>,
    /// Column → temporal constraint relative to a parent column.
    pub temporal_constraints: HashMap<String, TemporalConstraint>,
    /// Column → distribution that interpolates across the simulation window.
    pub timeline_overrides: HashMap<String, TimelineDistribution>,
}

impl TableLifecycle {
    /// A bare table that only carries a growth model.
    pub fn from_growth(growth: GrowthModel) -> Self {
        Self {
            growth,
            churn: None,
            seasonality: None,
            temporal_constraints: HashMap::new(),
            timeline_overrides: HashMap::new(),
        }
    }
}

/// Per-table counts for one bucket in a dry-run simulation.
#[derive(Debug, Clone, PartialEq)]
pub struct TableBucketStat {
    pub table: String,
    pub new: usize,
    pub churned: usize,
    pub active: usize,
}

/// One bucket's row of a dry-run plan.
#[derive(Debug, Clone)]
pub struct BucketPlan {
    pub label: String,
    pub stats: Vec<TableBucketStat>,
}

/// The output of a dry-run: per-bucket counts plus per-table totals, all derived
/// from the growth/churn/seasonality math without touching a database.
#[derive(Debug, Clone)]
pub struct SimulationReport {
    pub table_order: Vec<String>,
    pub buckets: Vec<BucketPlan>,
    /// table → (total ever created, still active at the end).
    pub totals: HashMap<String, (usize, usize)>,
}

/// Drives bucketed, time-evolving generation across all configured tables.
pub struct LifecycleEngine {
    pub config: LifecycleConfig,
    pub table_configs: HashMap<String, TableLifecycle>,
}

impl LifecycleEngine {
    pub fn new(config: LifecycleConfig, table_configs: HashMap<String, TableLifecycle>) -> Self {
        Self {
            config,
            table_configs,
        }
    }

    /// Expand the simulation window into ordered time buckets. The end date is
    /// exclusive: a bucket is emitted while its start is strictly before `end`.
    pub fn generate_buckets(&self) -> Vec<TimeBucket> {
        let mut buckets = Vec::new();
        let mut cursor = self.config.start;
        let mut index = 0;
        while cursor < self.config.end {
            let end = advance(cursor, self.config.bucket);
            // Guard against a non-advancing step (e.g. degenerate granularity).
            if end <= cursor {
                break;
            }
            buckets.push(TimeBucket {
                index,
                start: cursor,
                end,
            });
            cursor = end;
            index += 1;
        }
        buckets
    }

    /// New entity count for a table in a bucket: growth scaled by seasonality.
    ///
    /// For cumulative growth curves the new count is the delta between this
    /// bucket's target population and the previous one (tracked in
    /// `prev_targets`); for `follows`/`custom` the model already yields a
    /// per-bucket count, consulting the active parent count for `follows`.
    fn new_count(
        &self,
        table_name: &str,
        table: &TableLifecycle,
        bucket: &TimeBucket,
        pools: &HashMap<String, EntityPool>,
        prev_targets: &mut HashMap<String, usize>,
        rng: &mut ChaCha8Rng,
    ) -> usize {
        let base = if table.growth.is_cumulative() {
            let target = table.growth.count_at(bucket.index, None, rng);
            let prev = prev_targets
                .insert(table_name.to_string(), target)
                .unwrap_or(0);
            target.saturating_sub(prev)
        } else {
            let active_parent = match &table.growth {
                GrowthModel::Follows { parent_table, .. } => {
                    pools.get(parent_table).map(|p| p.active_count())
                }
                _ => None,
            };
            table.growth.count_at(bucket.index, active_parent, rng)
        };
        let seasonal = table
            .seasonality
            .as_ref()
            .map(|s| s.multiplier_for(bucket))
            .unwrap_or(1.0);
        (base as f64 * seasonal).round() as usize
    }

    /// Build the per-bucket [`GenerateConfig`] for a table: the row count plus the
    /// timestamp/distribution constraints derived from the bucket. The existing
    /// generation core consumes the row count today; the column overrides express
    /// the engine's intent (bucket-windowed creation times, interpolated
    /// distributions, parent-relative temporal links) for when generation honors
    /// per-column overrides.
    pub fn build_bucket_config(
        &self,
        table_name: &str,
        table: &TableLifecycle,
        bucket: &TimeBucket,
        new_count: usize,
        base_config: &GenerateConfig,
    ) -> GenerateConfig {
        let mut overrides: HashMap<String, ColumnOverride> = HashMap::new();

        // Root tables: constrain the creation timestamp to the bucket window.
        if !matches!(table.growth, GrowthModel::Follows { .. }) {
            let (min, max) = bucket_day_range(bucket);
            overrides.insert(
                BUCKET_TIME_COLUMN.to_string(),
                ColumnOverride::Range { min, max },
            );
        }

        // Distributions that evolve over time → snapshot at this bucket's start.
        for (column, timeline) in &table.timeline_overrides {
            let dist = timeline.distribution_at(bucket.start);
            overrides.insert(column.clone(), ColumnOverride::Distribution(dist));
        }

        // Temporal links to a parent column. (Offset is carried by the engine's
        // TemporalConstraint, not by the coarser ColumnOverride representation.)
        for (column, constraint) in &table.temporal_constraints {
            let ov = match constraint {
                TemporalConstraint::After { table, column, .. }
                | TemporalConstraint::Before { table, column, .. } => ColumnOverride::AfterParent {
                    parent_table: table.clone(),
                    parent_column: column.clone(),
                },
                TemporalConstraint::Equals { table, column } => ColumnOverride::FromParent {
                    parent_table: table.clone(),
                    parent_column: column.clone(),
                },
            };
            overrides.insert(column.clone(), ov);
        }

        let mut tables = HashMap::new();
        tables.insert(
            table_name.to_string(),
            TableScenario {
                count: CountExpression::Fixed(new_count),
                overrides,
            },
        );

        GenerateConfig {
            seed: base_config.seed,
            rows_per_table: new_count,
            scenario: Some(ScenarioConfig {
                seed: Some(base_config.seed),
                tables,
                ..Default::default()
            }),
            output_mode: base_config.output_mode.clone(),
            include_tables: base_config.include_tables.clone(),
            exclude_tables: base_config.exclude_tables.clone(),
            truncate_first: false,
        }
    }

    /// Run the simulation: for each bucket, in topological table order, compute
    /// counts, apply churn, generate the bucket's new rows via the existing
    /// `generate_table`, and track the resulting entities.
    pub async fn execute(
        &self,
        schema: &SchemaGraph,
        plan: &InsertionPlan,
        base_config: &GenerateConfig,
        pool: &PgPool,
    ) -> Result<GenerationResult, GenerateError> {
        let started = Instant::now();
        let mut rng = ChaCha8Rng::seed_from_u64(base_config.seed);
        let buckets = self.generate_buckets();

        let mut entity_pools: HashMap<String, EntityPool> = HashMap::new();
        let mut generated_ids: HashMap<String, Vec<Value>> = HashMap::new();
        // Persisted across buckets so UNIQUE columns stay unique over the whole
        // table lifetime, not just within a single bucket's generation call.
        let mut unique_states: HashMap<String, UniqueState> = HashMap::new();
        let mut prev_targets: HashMap<String, usize> = HashMap::new();
        let mut rows_per_table: HashMap<String, usize> = HashMap::new();
        let mut dur_per_table: HashMap<String, Duration> = HashMap::new();
        let mut total_rows = 0usize;

        let mut sql_file: Option<File> = match &base_config.output_mode {
            OutputMode::SqlFile(path) => Some(File::create(path)?),
            _ => None,
        };

        for bucket in &buckets {
            for table_name in &plan.ordered_tables {
                let Some(table_lc) = self.table_configs.get(table_name) else {
                    continue;
                };
                let Some(table) = schema.table(table_name) else {
                    continue;
                };

                let table_started = Instant::now();
                let new_count = self.new_count(
                    table_name,
                    table_lc,
                    bucket,
                    &entity_pools,
                    &mut prev_targets,
                    &mut rng,
                );

                // 1. Churn existing entities, emitting UPDATEs in the active mode.
                if let Some(churn) = &table_lc.churn {
                    if let Some(entity_pool) = entity_pools.get_mut(table_name) {
                        let events = churn.apply(&entity_pool.entities, bucket, &mut rng);
                        let updates = entity_pool.apply_churn(&events, churn);

                        // Cascade: drop churned ids from the FK pool so later
                        // buckets stop generating children for them.
                        if churn.cascade && !events.is_empty() {
                            let churned: HashSet<i64> =
                                events.iter().map(|e| e.entity_id).collect();
                            if let Some(ids) = generated_ids.get_mut(table_name) {
                                ids.retain(|v| !matches!(v, Value::Int(i) if churned.contains(i)));
                            }
                        }

                        match &base_config.output_mode {
                            OutputMode::DirectInsert => {
                                for stmt in &updates {
                                    sqlx::raw_sql(stmt).execute(pool).await?;
                                }
                                // Record per-entity churn time in the soft-delete
                                // column when the table has one.
                                if table.columns.iter().any(|c| c.name == CHURN_TIME_COLUMN) {
                                    let stamps: Vec<(i64, NaiveDateTime)> = events
                                        .iter()
                                        .map(|e| (e.entity_id, e.churned_at))
                                        .collect();
                                    update_timestamp_column(
                                        pool,
                                        table_name,
                                        CHURN_TIME_COLUMN,
                                        &stamps,
                                    )
                                    .await?;
                                }
                            }
                            OutputMode::SqlFile(_) => {
                                if let Some(f) = sql_file.as_mut() {
                                    for stmt in &updates {
                                        f.write_all(stmt.as_bytes())?;
                                        f.write_all(b"\n")?;
                                    }
                                }
                            }
                            OutputMode::Stdout => {
                                for stmt in &updates {
                                    println!("{stmt}");
                                }
                            }
                            OutputMode::Json(_) => {}
                        }
                    }
                }

                // 2. Build the per-bucket config; generation consumes its row count.
                let bucket_config =
                    self.build_bucket_config(table_name, table_lc, bucket, new_count, base_config);
                let row_count = bucket_config.rows_per_table;

                // 3. Generate the bucket's new rows via the EXISTING core.
                let prev_len = generated_ids.get(table_name).map(|v| v.len()).unwrap_or(0);
                let outcome = generate_table(
                    pool,
                    schema,
                    table,
                    row_count,
                    &plan.deferred_updates,
                    &mut rng,
                    &mut generated_ids,
                    &mut unique_states,
                    &base_config.output_mode,
                )
                .await?;

                if let Some(sql) = outcome.sql {
                    match &base_config.output_mode {
                        OutputMode::SqlFile(_) => {
                            if let Some(f) = sql_file.as_mut() {
                                f.write_all(sql.as_bytes())?;
                            }
                        }
                        OutputMode::Stdout => print!("{sql}"),
                        _ => {}
                    }
                }

                // 4. Track the newly generated entities and write their real
                //    creation timestamps (bucket-windowed for roots, parent-
                //    relative for children) into the database.
                let new_ids: Vec<Value> = generated_ids
                    .get(table_name)
                    .map(|v| v[prev_len..].to_vec())
                    .unwrap_or_default();
                let new_int_ids: Vec<i64> = new_ids
                    .iter()
                    .filter_map(|v| match v {
                        Value::Int(i) => Some(*i),
                        _ => None,
                    })
                    .collect();
                let direct = matches!(base_config.output_mode, OutputMode::DirectInsert);

                // Compute each new entity's created_at.
                let pairs: Vec<(i64, NaiveDateTime)> = {
                    let temporal = table_lc.temporal_constraints.get(BUCKET_TIME_COLUMN);
                    match temporal {
                        Some(constraint) if direct => {
                            let parent_table = constraint.parent_table().to_string();
                            let parent_map = match fk_column(schema, table_name, &parent_table) {
                                Some(fk) => {
                                    read_parent_map(pool, table_name, fk, &new_int_ids).await?
                                }
                                None => HashMap::new(),
                            };
                            let parent_pool = entity_pools.get(&parent_table);
                            let mut pairs = Vec::with_capacity(new_int_ids.len());
                            for id in &new_int_ids {
                                let parent_ts = parent_map
                                    .get(id)
                                    .and_then(|pid| parent_pool.and_then(|pp| pp.entity(*pid)))
                                    .map(|pe| pe.created_at);
                                let ts = match parent_ts {
                                    Some(pts) => clamp_temporal(constraint, pts, bucket, &mut rng),
                                    None => bucket.random_datetime(&mut rng),
                                };
                                pairs.push((*id, ts));
                            }
                            pairs
                        }
                        _ => new_int_ids
                            .iter()
                            .map(|id| (*id, bucket.random_datetime(&mut rng)))
                            .collect(),
                    }
                };

                entity_pools
                    .entry(table_name.clone())
                    .or_insert_with(|| EntityPool::new(table_name.clone()))
                    .add_entities_with(&pairs, bucket.index);

                // Persist timestamps and reset the churn column for new rows so
                // freshly created entities start active (overriding the random
                // value the generator produced).
                if direct {
                    let has_time_col = table.columns.iter().any(|c| c.name == BUCKET_TIME_COLUMN);
                    if has_time_col {
                        update_timestamp_column(pool, table_name, BUCKET_TIME_COLUMN, &pairs)
                            .await?;
                    }
                    if let Some(churn) = &table_lc.churn {
                        if let Value::Bool(churned_value) = churn.value {
                            update_bool_column(
                                pool,
                                table_name,
                                &churn.column,
                                !churned_value,
                                &new_int_ids,
                            )
                            .await?;
                        }
                        // New rows start un-churned: clear any soft-delete stamp
                        // the generator may have produced.
                        if table.columns.iter().any(|c| c.name == CHURN_TIME_COLUMN) {
                            null_column(pool, table_name, CHURN_TIME_COLUMN, &new_int_ids).await?;
                        }
                    }
                }

                *rows_per_table.entry(table_name.clone()).or_insert(0) += outcome.inserted;
                *dur_per_table
                    .entry(table_name.clone())
                    .or_insert(Duration::ZERO) += table_started.elapsed();
                total_rows += outcome.inserted;
            }
        }

        let tables_seeded: Vec<TableResult> = plan
            .ordered_tables
            .iter()
            .filter(|t| self.table_configs.contains_key(*t))
            .map(|t| TableResult {
                name: t.clone(),
                rows_inserted: rows_per_table.get(t).copied().unwrap_or(0),
                duration: dur_per_table.get(t).copied().unwrap_or(Duration::ZERO),
            })
            .collect();

        Ok(GenerationResult {
            tables_seeded,
            total_rows,
            duration: started.elapsed(),
            seed_used: base_config.seed,
        })
    }

    /// Simulate the plan without a database: runs the same growth/churn/
    /// seasonality logic as `execute` but assigns synthetic sequential ids
    /// instead of generating rows. Used by `--dry-run`.
    pub fn simulate(&self, seed: u64) -> SimulationReport {
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        let buckets = self.generate_buckets();
        let order = self.simulation_order();

        let mut pools: HashMap<String, EntityPool> = HashMap::new();
        let mut next_id: HashMap<String, i64> = HashMap::new();
        let mut prev_targets: HashMap<String, usize> = HashMap::new();
        let mut bucket_plans = Vec::with_capacity(buckets.len());

        for bucket in &buckets {
            let mut stats = Vec::with_capacity(order.len());
            for table_name in &order {
                let table_lc = &self.table_configs[table_name];

                let new_count = self.new_count(
                    table_name,
                    table_lc,
                    bucket,
                    &pools,
                    &mut prev_targets,
                    &mut rng,
                );

                let churned = match (&table_lc.churn, pools.get_mut(table_name)) {
                    (Some(churn), Some(pool)) => {
                        let events = churn.apply(&pool.entities, bucket, &mut rng);
                        let n = events.len();
                        pool.apply_churn(&events, churn);
                        n
                    }
                    _ => 0,
                };

                let start_id = next_id.entry(table_name.clone()).or_insert(1);
                let ids: Vec<Value> = (*start_id..*start_id + new_count as i64)
                    .map(Value::Int)
                    .collect();
                *start_id += new_count as i64;

                let pool = pools
                    .entry(table_name.clone())
                    .or_insert_with(|| EntityPool::new(table_name.clone()));
                pool.add_entities(&ids, bucket, &mut rng);

                stats.push(TableBucketStat {
                    table: table_name.clone(),
                    new: new_count,
                    churned,
                    active: pool.active_count(),
                });
            }
            bucket_plans.push(BucketPlan {
                label: bucket_label(bucket, self.config.bucket),
                stats,
            });
        }

        let totals = order
            .iter()
            .map(|t| {
                let pool = pools.get(t);
                (
                    t.clone(),
                    (
                        pool.map(|p| p.total_count()).unwrap_or(0),
                        pool.map(|p| p.active_count()).unwrap_or(0),
                    ),
                )
            })
            .collect();

        SimulationReport {
            table_order: order,
            buckets: bucket_plans,
            totals,
        }
    }

    /// Order tables so a `follows` child comes after its parent. Ties (and roots)
    /// are broken by name for determinism. Used when no DB schema is available.
    fn simulation_order(&self) -> Vec<String> {
        let mut names: Vec<String> = self.table_configs.keys().cloned().collect();
        names.sort();

        let mut ordered = Vec::with_capacity(names.len());
        let mut placed: HashSet<String> = HashSet::new();
        loop {
            let mut progressed = false;
            for name in &names {
                if placed.contains(name) {
                    continue;
                }
                let ready = match &self.table_configs[name].growth {
                    GrowthModel::Follows { parent_table, .. } => {
                        placed.contains(parent_table)
                            || !self.table_configs.contains_key(parent_table)
                    }
                    _ => true,
                };
                if ready {
                    ordered.push(name.clone());
                    placed.insert(name.clone());
                    progressed = true;
                }
            }
            if ordered.len() == names.len() {
                break;
            }
            if !progressed {
                // Cycle or dangling parent — append the rest deterministically.
                for name in &names {
                    if placed.insert(name.clone()) {
                        ordered.push(name.clone());
                    }
                }
                break;
            }
        }
        ordered
    }
}

/// A short human label for a bucket, scaled to its granularity.
fn bucket_label(bucket: &TimeBucket, granularity: BucketGranularity) -> String {
    match granularity {
        BucketGranularity::Month | BucketGranularity::Quarter => {
            bucket.start.format("%Y-%m").to_string()
        }
        BucketGranularity::Day | BucketGranularity::Week => {
            bucket.start.format("%Y-%m-%d").to_string()
        }
    }
}

/// Resolve a child timestamp from its parent's, then keep it inside the bucket
/// window so that per-bucket effects (e.g. seasonality) are visible by
/// `created_at`. The lower bound is `max(bucket.start, parent_ts)`, so the
/// "after parent" guarantee is never violated even when parent and child land in
/// the same bucket. `Equals` is returned exactly (no clamp).
fn clamp_temporal(
    constraint: &TemporalConstraint,
    parent_ts: NaiveDateTime,
    bucket: &TimeBucket,
    rng: &mut ChaCha8Rng,
) -> NaiveDateTime {
    let resolved = constraint.resolve(parent_ts, rng);
    match constraint {
        TemporalConstraint::Equals { .. } => resolved,
        _ => {
            let lower = parent_ts.max(bucket.start_datetime());
            let upper = (bucket.end_datetime() - chrono::Duration::seconds(1)).max(lower);
            resolved.clamp(lower, upper)
        }
    }
}

/// The FK column on `child` that references `parent`, if any.
fn fk_column<'a>(schema: &'a SchemaGraph, child: &str, parent: &str) -> Option<&'a str> {
    schema
        .foreign_keys
        .iter()
        .find(|fk| fk.from_table == child && fk.to_table == parent)
        .map(|fk| fk.from_column.as_str())
}

/// Read back the parent id (`fk_col`) for each of `ids` in `table`.
/// Rows with a NULL FK are omitted.
async fn read_parent_map(
    pool: &PgPool,
    table: &str,
    fk_col: &str,
    ids: &[i64],
) -> Result<HashMap<i64, i64>, GenerateError> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let sql = format!(
        "SELECT \"id\"::int8 AS id, {}::int8 AS pid FROM {} WHERE \"id\" = ANY($1::int8[])",
        quote_ident(fk_col),
        quote_ident(table),
    );
    let rows = sqlx::query(&sql).bind(ids).fetch_all(pool).await?;
    let mut map = HashMap::with_capacity(rows.len());
    for row in rows {
        let id: i64 = row.try_get("id")?;
        let pid: Option<i64> = row.try_get("pid")?;
        if let Some(pid) = pid {
            map.insert(id, pid);
        }
    }
    Ok(map)
}

/// Set a timestamp column for many rows in one batched UPDATE (chunked to stay
/// well under PostgreSQL's parameter limit).
async fn update_timestamp_column(
    pool: &PgPool,
    table: &str,
    column: &str,
    pairs: &[(i64, NaiveDateTime)],
) -> Result<(), GenerateError> {
    for chunk in pairs.chunks(1000) {
        if chunk.is_empty() {
            continue;
        }
        let values = (0..chunk.len())
            .map(|i| format!("(${}::int8, ${}::timestamp)", i * 2 + 1, i * 2 + 2))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "UPDATE {} AS t SET {} = v.ts FROM (VALUES {}) AS v(id, ts) WHERE t.\"id\" = v.id",
            quote_ident(table),
            quote_ident(column),
            values,
        );
        let mut query = sqlx::query(&sql);
        for (id, ts) in chunk {
            query = query.bind(*id).bind(*ts);
        }
        query.execute(pool).await?;
    }
    Ok(())
}

/// Set a boolean column to `value` for the given ids (used to reset newly
/// created rows to the active state before churn flips a subset).
async fn update_bool_column(
    pool: &PgPool,
    table: &str,
    column: &str,
    value: bool,
    ids: &[i64],
) -> Result<(), GenerateError> {
    if ids.is_empty() {
        return Ok(());
    }
    let sql = format!(
        "UPDATE {} SET {} = $1 WHERE \"id\" = ANY($2::int8[])",
        quote_ident(table),
        quote_ident(column),
    );
    sqlx::query(&sql)
        .bind(value)
        .bind(ids)
        .execute(pool)
        .await?;
    Ok(())
}

/// Set a column to NULL for the given ids.
async fn null_column(
    pool: &PgPool,
    table: &str,
    column: &str,
    ids: &[i64],
) -> Result<(), GenerateError> {
    if ids.is_empty() {
        return Ok(());
    }
    let sql = format!(
        "UPDATE {} SET {} = NULL WHERE \"id\" = ANY($1::int8[])",
        quote_ident(table),
        quote_ident(column),
    );
    sqlx::query(&sql).bind(ids).execute(pool).await?;
    Ok(())
}

/// Advance a date by one bucket of the given granularity.
fn advance(date: NaiveDate, granularity: BucketGranularity) -> NaiveDate {
    match granularity {
        BucketGranularity::Day => date.succ_opt().unwrap_or(date),
        BucketGranularity::Week => date + chrono::Duration::days(7),
        BucketGranularity::Month => add_months(date, 1),
        BucketGranularity::Quarter => add_months(date, 3),
    }
}

/// Add `n` calendar months to `date`, clamping the day to the target month's
/// length. Falls back to the original date if construction somehow fails.
fn add_months(date: NaiveDate, n: u32) -> NaiveDate {
    let zero_based = date.month0() as i64 + n as i64;
    let year = date.year() + (zero_based.div_euclid(12)) as i32;
    let month = (zero_based.rem_euclid(12)) as u32 + 1;
    let day = date.day().min(last_day_of_month(year, month));
    NaiveDate::from_ymd_opt(year, month, day).unwrap_or(date)
}

/// Number of days in `month` of `year`.
fn last_day_of_month(year: i32, month: u32) -> u32 {
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    match (
        NaiveDate::from_ymd_opt(next_year, next_month, 1),
        NaiveDate::from_ymd_opt(year, month, 1),
    ) {
        (Some(next), Some(_)) => next.pred_opt().map(|d| d.day()).unwrap_or(28),
        _ => 28,
    }
}

/// Bucket `[start, end)` expressed as a day-since-epoch range, matching the
/// representation the scenario parser uses for date ranges.
fn bucket_day_range(bucket: &TimeBucket) -> (f64, f64) {
    match NaiveDate::from_ymd_opt(1970, 1, 1) {
        Some(epoch) => (
            (bucket.start - epoch).num_days() as f64,
            (bucket.end - epoch).num_days() as f64,
        ),
        None => (0.0, 0.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).expect("valid date")
    }

    fn engine(start: NaiveDate, end: NaiveDate, bucket: BucketGranularity) -> LifecycleEngine {
        LifecycleEngine::new(LifecycleConfig { start, end, bucket }, HashMap::new())
    }

    #[test]
    fn test_generate_buckets_monthly() {
        let e = engine(date(2024, 1, 1), date(2024, 4, 1), BucketGranularity::Month);
        let buckets = e.generate_buckets();
        assert_eq!(buckets.len(), 3); // Jan, Feb, Mar (end exclusive)
        assert_eq!(buckets[0].start, date(2024, 1, 1));
        assert_eq!(buckets[0].end, date(2024, 2, 1));
        assert_eq!(buckets[2].start, date(2024, 3, 1));
        assert_eq!(buckets[2].end, date(2024, 4, 1));
        assert_eq!(buckets[2].index, 2);
    }

    #[test]
    fn test_generate_buckets_monthly_crosses_year_boundary() {
        let e = engine(
            date(2023, 11, 1),
            date(2024, 2, 1),
            BucketGranularity::Month,
        );
        let buckets = e.generate_buckets();
        assert_eq!(buckets.len(), 3); // Nov 2023, Dec 2023, Jan 2024
        assert_eq!(buckets[1].start, date(2023, 12, 1));
        assert_eq!(buckets[2].start, date(2024, 1, 1));
    }

    #[test]
    fn test_generate_buckets_daily() {
        let e = engine(date(2024, 1, 1), date(2024, 1, 5), BucketGranularity::Day);
        let buckets = e.generate_buckets();
        assert_eq!(buckets.len(), 4);
        assert_eq!(buckets[3].start, date(2024, 1, 4));
        assert_eq!(buckets[3].end, date(2024, 1, 5));
    }

    #[test]
    fn test_generate_buckets_weekly() {
        let e = engine(date(2024, 1, 1), date(2024, 1, 29), BucketGranularity::Week);
        let buckets = e.generate_buckets();
        assert_eq!(buckets.len(), 4); // 4 weeks of 7 days
        assert_eq!(buckets[1].start, date(2024, 1, 8));
    }

    #[test]
    fn test_generate_buckets_quarterly() {
        let e = engine(
            date(2024, 1, 1),
            date(2025, 1, 1),
            BucketGranularity::Quarter,
        );
        let buckets = e.generate_buckets();
        assert_eq!(buckets.len(), 4);
        assert_eq!(buckets[0].end, date(2024, 4, 1));
        assert_eq!(buckets[1].start, date(2024, 4, 1));
        assert_eq!(buckets[3].start, date(2024, 10, 1));
    }

    #[test]
    fn test_generate_buckets_empty_when_start_after_end() {
        let e = engine(date(2024, 5, 1), date(2024, 1, 1), BucketGranularity::Month);
        assert!(e.generate_buckets().is_empty());
    }

    #[test]
    fn test_add_months_clamps_day() {
        // Jan 31 + 1 month → Feb 29 (2024 is a leap year).
        assert_eq!(add_months(date(2024, 1, 31), 1), date(2024, 2, 29));
        // Jan 31 + 1 month in a non-leap year → Feb 28.
        assert_eq!(add_months(date(2023, 1, 31), 1), date(2023, 2, 28));
    }

    #[test]
    fn test_build_bucket_config_root_sets_count_and_bucket_range() {
        let e = engine(date(2024, 1, 1), date(2024, 4, 1), BucketGranularity::Month);
        let table = TableLifecycle::from_growth(GrowthModel::Linear {
            initial: 10.0,
            rate: 5.0,
        });
        let bucket = TimeBucket {
            index: 0,
            start: date(2024, 1, 1),
            end: date(2024, 2, 1),
        };
        let base = GenerateConfig::default();

        let cfg = e.build_bucket_config("users", &table, &bucket, 42, &base);
        assert_eq!(cfg.rows_per_table, 42);

        let scenario = cfg.scenario.expect("scenario present");
        let ts = scenario.tables.get("users").expect("users scenario");
        assert_eq!(ts.count, CountExpression::Fixed(42));

        match ts.overrides.get(BUCKET_TIME_COLUMN) {
            Some(ColumnOverride::Range { min, max }) => {
                // 2024-01-01 .. 2024-02-01 as days since epoch.
                let (e_min, e_max) = bucket_day_range(&bucket);
                assert_eq!(*min, e_min);
                assert_eq!(*max, e_max);
            }
            other => panic!("expected a bucket Range override, got {other:?}"),
        }
    }

    #[test]
    fn test_build_bucket_config_follows_has_no_bucket_range() {
        let e = engine(date(2024, 1, 1), date(2024, 4, 1), BucketGranularity::Month);
        let table = TableLifecycle::from_growth(GrowthModel::Follows {
            parent_table: "users".into(),
            ratio: Some(3.0),
            per_parent: None,
            variance: 0.0,
        });
        let bucket = TimeBucket {
            index: 0,
            start: date(2024, 1, 1),
            end: date(2024, 2, 1),
        };
        let base = GenerateConfig::default();

        let cfg = e.build_bucket_config("orders", &table, &bucket, 99, &base);
        let scenario = cfg.scenario.expect("scenario present");
        let ts = scenario.tables.get("orders").expect("orders scenario");
        // Child tables time themselves relative to a parent, not the bucket.
        assert!(!ts.overrides.contains_key(BUCKET_TIME_COLUMN));
    }
}
