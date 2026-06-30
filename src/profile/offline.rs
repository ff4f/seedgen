//! Offline profiling (Security Model — Layer 5).
//!
//! For organizations that won't let an external tool touch production:
//!
//! 1. `export_collection_sql` emits a single read-only SQL statement that, when
//!    run by a DBA, produces one self-describing JSON document of *aggregate*
//!    results. The categorical-distribution sub-queries are **self-guarded** —
//!    they return values only for low-cardinality columns, so high-cardinality
//!    values never leave the database even in offline mode.
//! 2. `import_results` reassembles a [`DatabaseProfile`] from that JSON, applying
//!    the same cardinality guard the live collector uses. No database connection
//!    is required.

use std::collections::{BTreeMap, HashSet};

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::introspection::SchemaGraph;
use crate::profile::collector::compute_source_hash;
use crate::profile::config::{ProfileOptions, ProfileOptionsSummary};
use crate::profile::errors::ProfileError;
use crate::profile::output::SUPPORTED_VERSION;
use crate::profile::queries::{is_serial, PlannedQuery, QueryBuilder, QueryKind};
use crate::profile::sensitive::{is_excluded, is_included, is_sensitive_column};
use crate::profile::stats::{
    ColumnProfile, DatabaseProfile, ParentRatio, Percentiles, TableProfile,
};

// ===========================================================================
// Export
// ===========================================================================

/// Build the single read-only SQL statement whose result is a self-describing
/// JSON document of all profiling results.
pub fn export_collection_sql(schema: &SchemaGraph, options: &ProfileOptions) -> String {
    let queries = QueryBuilder::new(schema, options).build_all();
    let threshold = options.cardinality_threshold;

    let entries: Vec<String> = queries
        .iter()
        .map(|q| {
            let col = q
                .column
                .as_deref()
                .map(|c| format!("'{}'", esc(c)))
                .unwrap_or_else(|| "NULL".to_string());
            let parent = q
                .parent_table
                .as_deref()
                .map(|p| format!("'{}'", esc(p)))
                .unwrap_or_else(|| "NULL".to_string());
            format!(
                "jsonb_build_object('kind','{kind:?}','table','{table}','column',{col},'parent',{parent},'rows',{rows})",
                kind = q.kind,
                table = esc(&q.table),
                rows = wrap_rows(q, threshold),
            )
        })
        .collect();

    let skipped = compute_skipped_sensitive(schema, options);
    let skipped_json = serde_json::to_string(&skipped).unwrap_or_else(|_| "[]".to_string());

    let header = "\
-- SeedGen offline profiling collection query (read-only, aggregate-only).
-- Run it and capture the single JSON value, e.g.:
--   psql \"$URL\" -At -f collect.sql -o results.json
-- Then build the profile with no production access:
--   seedgen profile --import-results results.json --output profile.yaml\n";

    format!(
        "{header}SELECT jsonb_build_object(\
         'version','{version}',\
         'source_hash','{source_hash}',\
         'seedgen_version','{seedgen_version}',\
         'options',jsonb_build_object(\
         'cardinality_threshold',{threshold},\
         'capture_percentiles',{capture_percentiles},\
         'skipped_sensitive','{skipped}'::jsonb),\
         'results',jsonb_build_array({entries})\
         ) AS profile_results;\n",
        version = SUPPORTED_VERSION,
        source_hash = esc(&compute_source_hash(schema)),
        seedgen_version = esc(env!("CARGO_PKG_VERSION")),
        capture_percentiles = options.capture_percentiles,
        skipped = esc(&skipped_json),
        entries = entries.join(","),
    )
}

/// Wrap a query so it yields a JSON array of its rows. Distribution queries are
/// self-guarded: nothing is returned for high-cardinality columns.
fn wrap_rows(q: &PlannedQuery, threshold: usize) -> String {
    let base = format!(
        "(SELECT coalesce(jsonb_agg(_r), '[]'::jsonb) FROM ({}) _r",
        q.sql
    );
    if matches!(q.kind, QueryKind::CategoricalDistribution) {
        if let Some(col) = &q.column {
            return format!(
                "{base} WHERE (SELECT COUNT(DISTINCT {c}) FROM {t}) <= {threshold})",
                c = quote_ident(col),
                t = quote_ident(&q.table),
            );
        }
    }
    format!("{base})")
}

/// Columns the live collector would skip as sensitive — embedded so the imported
/// profile's `skipped_sensitive` summary matches a live profile.
fn compute_skipped_sensitive(schema: &SchemaGraph, options: &ProfileOptions) -> Vec<String> {
    let mut out = Vec::new();
    for table in &schema.tables {
        let fk_columns: HashSet<&str> = schema
            .foreign_keys
            .iter()
            .filter(|fk| fk.from_table == table.name)
            .map(|fk| fk.from_column.as_str())
            .collect();
        for column in &table.columns {
            if is_serial(column)
                || column.is_generated
                || fk_columns.contains(column.name.as_str())
                || is_excluded(&table.name, &column.name, &options.exclude_columns)
            {
                continue;
            }
            if is_sensitive_column(&column.name)
                && !is_included(&table.name, &column.name, &options.include_columns)
            {
                out.push(format!("{}.{}", table.name, column.name));
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

fn esc(s: &str) -> String {
    s.replace('\'', "''")
}

fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

// ===========================================================================
// Import
// ===========================================================================

#[derive(Debug, Deserialize)]
struct ResultsDoc {
    version: String,
    source_hash: String,
    seedgen_version: String,
    options: OptionsDoc,
    results: Vec<ResultEntry>,
}

#[derive(Debug, Deserialize)]
struct OptionsDoc {
    cardinality_threshold: usize,
    #[serde(default = "default_true")]
    capture_percentiles: bool,
    #[serde(default)]
    skipped_sensitive: Vec<String>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
struct ResultEntry {
    kind: String,
    table: String,
    #[serde(default)]
    column: Option<String>,
    #[serde(default)]
    parent: Option<String>,
    #[serde(default)]
    rows: Vec<Map<String, Value>>,
}

/// Borrowed `kind → rows` map for one column, built while indexing the document.
type KindRows<'a> = BTreeMap<String, &'a Vec<Map<String, Value>>>;

/// Reassemble a [`DatabaseProfile`] from an externally-collected results
/// document. No database connection is required.
pub fn import_results(results_json: &str) -> Result<DatabaseProfile, ProfileError> {
    let doc: ResultsDoc = serde_json::from_str(results_json)?;
    if doc.version != SUPPORTED_VERSION {
        return Err(ProfileError::UnsupportedVersion {
            found: doc.version,
            expected: SUPPORTED_VERSION.to_string(),
        });
    }
    let threshold = doc.options.cardinality_threshold;
    let capture_percentiles = doc.options.capture_percentiles;

    // Per-table row counts (also used for parent zero-rate).
    let mut row_counts: BTreeMap<String, u64> = BTreeMap::new();
    for e in &doc.results {
        if e.kind == "RowCount" && e.column.is_none() {
            let n = e.rows.first().map(|r| ji64(r, "row_count").max(0) as u64);
            row_counts.insert(e.table.clone(), n.unwrap_or(0));
        }
    }

    // Group column-level results by (table, column) → kind → rows.
    let mut column_groups: BTreeMap<(String, String), KindRows> = BTreeMap::new();
    let mut all_tables: HashSet<String> = row_counts.keys().cloned().collect();
    for e in &doc.results {
        all_tables.insert(e.table.clone());
        if let Some(col) = &e.column {
            if is_column_kind(&e.kind) {
                column_groups
                    .entry((e.table.clone(), col.clone()))
                    .or_default()
                    .insert(e.kind.clone(), &e.rows);
            }
        }
    }

    let mut tables = BTreeMap::new();
    for table in &all_tables {
        let row_count = row_counts.get(table).copied().unwrap_or(0);

        let parent_ratios = build_parent_ratios(&doc.results, table, &row_counts);

        let mut columns = BTreeMap::new();
        for ((t, col), kinds) in &column_groups {
            if t != table {
                continue;
            }
            if let Some(profile) = assemble_column(kinds, threshold, capture_percentiles) {
                columns.insert(col.clone(), profile);
            }
        }

        tables.insert(
            table.clone(),
            TableProfile {
                row_count,
                parent_ratios,
                columns,
            },
        );
    }

    Ok(DatabaseProfile {
        version: SUPPORTED_VERSION.to_string(),
        profiled_at: chrono::Utc::now().to_rfc3339(),
        source_hash: doc.source_hash,
        seedgen_version: doc.seedgen_version,
        options: ProfileOptionsSummary {
            cardinality_threshold: threshold,
            skipped_sensitive: doc.options.skipped_sensitive,
        },
        tables,
    })
}

fn is_column_kind(kind: &str) -> bool {
    matches!(
        kind,
        "NumericStats"
            | "BooleanStats"
            | "TimestampStats"
            | "TimestampMonthly"
            | "TimestampHourly"
            | "CardinalityCheck"
            | "CategoricalDistribution"
            | "StringStats"
    )
}

fn build_parent_ratios(
    results: &[ResultEntry],
    child_table: &str,
    row_counts: &BTreeMap<String, u64>,
) -> BTreeMap<String, ParentRatio> {
    let mut ratios = BTreeMap::new();
    for e in results {
        if e.kind != "ParentRatio" || e.table != child_table {
            continue;
        }
        let parent = match &e.parent {
            Some(p) => p.clone(),
            None => continue,
        };
        let fk_column = e.column.clone().unwrap_or_default();
        let Some(r) = e.rows.first() else { continue };

        let zero_count = results
            .iter()
            .find(|z| {
                z.kind == "ParentZeroCount"
                    && z.table == child_table
                    && z.column == e.column
                    && z.parent == e.parent
            })
            .and_then(|z| z.rows.first())
            .map(|zr| ji64(zr, "zero_count").max(0) as u64);

        let parent_count = row_counts.get(&parent).copied().unwrap_or(0);
        let zero_rate = match zero_count {
            Some(zc) if parent_count > 0 => Some(zc as f64 * 100.0 / parent_count as f64),
            _ => None,
        };
        let median = jf64(r, "median_ratio");

        ratios.insert(
            parent,
            ParentRatio {
                column: fk_column,
                avg: jf64(r, "avg_ratio"),
                min: ji64(r, "min_ratio").max(0) as u64,
                max: ji64(r, "max_ratio").max(0) as u64,
                median,
                stddev: jf64(r, "stddev_ratio"),
                percentiles: Some(Percentiles {
                    p5: None,
                    p10: None,
                    p25: jf64(r, "p25_ratio"),
                    p50: median,
                    p75: jf64(r, "p75_ratio"),
                    p90: None,
                    p95: jf64(r, "p95_ratio"),
                    p99: jf64(r, "p99_ratio"),
                }),
                zero_count,
                zero_rate,
            },
        );
    }
    ratios
}

fn assemble_column(
    kinds: &KindRows,
    threshold: usize,
    capture_percentiles: bool,
) -> Option<ColumnProfile> {
    if let Some(r) = kinds.get("NumericStats").and_then(|rows| rows.first()) {
        let percentiles = capture_percentiles.then(|| Percentiles {
            p5: None,
            p10: None,
            p25: jf64(r, "p25"),
            p50: jf64(r, "p50"),
            p75: jf64(r, "p75"),
            p90: None,
            p95: jf64(r, "p95"),
            p99: jf64(r, "p99"),
        });
        return Some(ColumnProfile::Numeric {
            min: jf64(r, "min_val"),
            max: jf64(r, "max_val"),
            mean: jf64(r, "mean_val"),
            median: jf64(r, "p50"),
            stddev: jf64(r, "stddev_val"),
            null_rate: jf64(r, "null_rate"),
            percentiles,
        });
    }

    if let Some(r) = kinds.get("BooleanStats").and_then(|rows| rows.first()) {
        return Some(ColumnProfile::Boolean {
            true_rate: jf64(r, "true_rate"),
            null_rate: jf64(r, "null_rate"),
        });
    }

    if let Some(r) = kinds.get("TimestampStats").and_then(|rows| rows.first()) {
        let mut monthly_density = BTreeMap::new();
        if let Some(rows) = kinds.get("TimestampMonthly") {
            for m in *rows {
                monthly_density.insert(jstr(m, "month"), ji64(m, "cnt").max(0) as u64);
            }
        }
        let mut hourly_density = BTreeMap::new();
        if let Some(rows) = kinds.get("TimestampHourly") {
            for h in *rows {
                hourly_density.insert(ji64(h, "hour").clamp(0, 23) as u8, jf64(h, "pct"));
            }
        }
        return Some(ColumnProfile::Timestamp {
            range: (jstr(r, "min_ts"), jstr(r, "max_ts")),
            null_rate: jf64(r, "null_rate"),
            weekday_ratio: jopt_f64(r, "weekday_ratio"),
            hourly_density,
            monthly_density,
        });
    }

    if let Some(cr) = kinds.get("CardinalityCheck").and_then(|rows| rows.first()) {
        let distinct = ji64(cr, "distinct_count").max(0) as u64;
        let null_rate = jf64(cr, "null_rate");
        let has_string = kinds.contains_key("StringStats");

        // High-cardinality string → aggregate stats only (cardinality guard).
        if has_string && distinct as usize > threshold {
            let sr = kinds.get("StringStats").and_then(|rows| rows.first())?;
            return Some(ColumnProfile::StringStats {
                semantic: None,
                cardinality: ji64(sr, "cardinality").max(0) as u64,
                null_rate: jf64(sr, "null_rate"),
                avg_length: jf64(sr, "avg_length"),
                min_length: jopt_u32(sr, "min_length"),
                max_length: jopt_u32(sr, "max_length"),
            });
        }

        // Low-cardinality string or enum → captured distribution.
        let mut distribution = BTreeMap::new();
        if let Some(rows) = kinds.get("CategoricalDistribution") {
            for d in *rows {
                distribution.insert(jstr(d, "value"), jf64(d, "pct"));
            }
        }
        return Some(ColumnProfile::Categorical {
            distribution,
            null_rate,
        });
    }

    None
}

// --- JSON row accessors ---

fn jf64(row: &Map<String, Value>, key: &str) -> f64 {
    row.get(key).and_then(Value::as_f64).unwrap_or(0.0)
}

fn ji64(row: &Map<String, Value>, key: &str) -> i64 {
    row.get(key).and_then(Value::as_i64).unwrap_or(0)
}

fn jstr(row: &Map<String, Value>, key: &str) -> String {
    row.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn jopt_f64(row: &Map<String, Value>, key: &str) -> Option<f64> {
    row.get(key)
        .filter(|v| !v.is_null())
        .and_then(Value::as_f64)
}

fn jopt_u32(row: &Map<String, Value>, key: &str) -> Option<u32> {
    row.get(key)
        .filter(|v| !v.is_null())
        .and_then(Value::as_i64)
        .map(|n| n.max(0) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::introspection::{Column, DataType, ForeignKey, Table};

    fn col(name: &str, data_type: DataType) -> Column {
        Column {
            name: name.to_string(),
            data_type,
            is_nullable: true,
            is_identity: false,
            is_generated: false,
            default_value: None,
            max_length: None,
            numeric_precision: None,
            numeric_scale: None,
        }
    }

    fn schema() -> SchemaGraph {
        SchemaGraph {
            tables: vec![
                Table {
                    name: "users".into(),
                    columns: vec![
                        Column {
                            is_identity: true,
                            ..col("id", DataType::Integer)
                        },
                        col("role", DataType::Text),
                        col("password_hash", DataType::Text),
                    ],
                    constraints: vec![],
                },
                Table {
                    name: "orders".into(),
                    columns: vec![col("user_id", DataType::Integer)],
                    constraints: vec![],
                },
            ],
            foreign_keys: vec![ForeignKey {
                from_table: "orders".into(),
                from_column: "user_id".into(),
                to_table: "users".into(),
                to_column: "id".into(),
                is_nullable: false,
                is_deferrable: false,
            }],
            enums: vec![],
        }
    }

    #[test]
    fn test_export_sql_is_read_only_and_self_guarded() {
        let sql = export_collection_sql(&schema(), &ProfileOptions::default());
        assert!(sql.contains("jsonb_build_object"));
        assert!(sql.contains("'source_hash'"));
        // The sensitive column is recorded in the embedded skip list, never queried.
        assert!(sql.contains("users.password_hash"));
        assert!(!sql.contains("INSERT"));
        assert!(!sql.contains("UPDATE"));
        assert!(!sql.contains("DELETE"));
        // Distribution self-guard present.
        assert!(sql.contains("COUNT(DISTINCT"));
    }

    #[test]
    fn test_import_rejects_unknown_version() {
        let json = r#"{"version":"9.9","source_hash":"x","seedgen_version":"0",
            "options":{"cardinality_threshold":50},"results":[]}"#;
        assert!(matches!(
            import_results(json),
            Err(ProfileError::UnsupportedVersion { .. })
        ));
    }

    #[test]
    fn test_import_assembles_categorical_and_ratio() {
        let json = r#"{
          "version":"1.0","source_hash":"sha256:x","seedgen_version":"0.3.0",
          "options":{"cardinality_threshold":50,"capture_percentiles":true,"skipped_sensitive":["users.password_hash"]},
          "results":[
            {"kind":"RowCount","table":"users","column":null,"parent":null,"rows":[{"row_count":100}]},
            {"kind":"RowCount","table":"orders","column":null,"parent":null,"rows":[{"row_count":300}]},
            {"kind":"CardinalityCheck","table":"users","column":"role","parent":null,"rows":[{"distinct_count":2,"null_rate":0.0}]},
            {"kind":"CategoricalDistribution","table":"users","column":"role","parent":null,"rows":[{"value":"admin","pct":20.0},{"value":"user","pct":80.0}]},
            {"kind":"StringStats","table":"users","column":"role","parent":null,"rows":[{"cardinality":2,"null_rate":0.0,"avg_length":4.0,"min_length":4,"max_length":5}]},
            {"kind":"ParentRatio","table":"orders","column":"user_id","parent":"users","rows":[{"avg_ratio":3.0,"min_ratio":0,"max_ratio":9,"median_ratio":3.0,"stddev_ratio":1.0,"p25_ratio":1.0,"p75_ratio":4.0,"p95_ratio":8.0,"p99_ratio":9.0}]},
            {"kind":"ParentZeroCount","table":"orders","column":"user_id","parent":"users","rows":[{"zero_count":10}]}
          ]
        }"#;
        let profile = import_results(json).expect("import");

        assert_eq!(profile.tables["users"].row_count, 100);
        assert_eq!(
            profile.options.skipped_sensitive,
            vec!["users.password_hash"]
        );

        // 2 distinct ≤ 50 → categorical (not StringStats), values captured.
        match &profile.tables["users"].columns["role"] {
            ColumnProfile::Categorical { distribution, .. } => {
                assert_eq!(distribution["admin"], 20.0);
                assert_eq!(distribution["user"], 80.0);
            }
            other => panic!("expected Categorical, got {other:?}"),
        }

        let ratio = &profile.tables["orders"].parent_ratios["users"];
        assert_eq!(ratio.avg, 3.0);
        assert_eq!(ratio.zero_count, Some(10));
        // zero_rate = 10 / 100 (parent users) * 100 = 10%.
        assert_eq!(ratio.zero_rate, Some(10.0));
    }

    #[test]
    fn test_import_high_cardinality_is_string_stats() {
        let json = r#"{
          "version":"1.0","source_hash":"x","seedgen_version":"0",
          "options":{"cardinality_threshold":50},
          "results":[
            {"kind":"RowCount","table":"users","column":null,"parent":null,"rows":[{"row_count":100}]},
            {"kind":"CardinalityCheck","table":"users","column":"email","parent":null,"rows":[{"distinct_count":100,"null_rate":0.0}]},
            {"kind":"CategoricalDistribution","table":"users","column":"email","parent":null,"rows":[]},
            {"kind":"StringStats","table":"users","column":"email","parent":null,"rows":[{"cardinality":100,"null_rate":0.0,"avg_length":18.0,"min_length":8,"max_length":40}]}
          ]
        }"#;
        let profile = import_results(json).expect("import");
        assert!(matches!(
            profile.tables["users"].columns["email"],
            ColumnProfile::StringStats {
                cardinality: 100,
                ..
            }
        ));
    }
}
