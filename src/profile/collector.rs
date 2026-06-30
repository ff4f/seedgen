//! [`ProfileCollector`] — executes the read-only, aggregate-only profiling
//! queries against a database and assembles a [`DatabaseProfile`].
//!
//! It NEVER reads row-level data. The cardinality guard ensures individual
//! values are captured only for low-cardinality (enum-like) columns; sensitive
//! and excluded columns are skipped; every action is recorded in the audit log.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::time::Instant;

use chrono::Utc;
use sqlx::pool::PoolConnection;
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, Row};
use tracing::warn;

use crate::introspection::{Column, ForeignKey, SchemaGraph, Table};
use crate::profile::audit::AuditLog;
use crate::profile::config::{ProfileOptions, ProfileOptionsSummary};
use crate::profile::errors::ProfileError;
use crate::profile::queries::{column_category, is_serial, Category, PlannedQuery, QueryBuilder};
use crate::profile::sensitive::{
    is_excluded, is_included, is_sensitive_column, SENSITIVE_PATTERNS,
};
use crate::profile::stats::{
    ColumnProfile, DatabaseProfile, ParentRatio, Percentiles, TableProfile,
};

/// Reads statistical summaries from a database. Runs every query on a single,
/// dedicated connection configured as read-only with a statement timeout.
pub struct ProfileCollector {
    conn: PoolConnection<Postgres>,
    schema: SchemaGraph,
    options: ProfileOptions,
    audit_log: AuditLog,
    skipped_sensitive: Vec<String>,
}

impl ProfileCollector {
    /// Acquire a dedicated connection and apply safety settings:
    /// warn (or, in strict mode, refuse) on superuser; force read-only
    /// transactions; set a statement timeout. The pool is only borrowed to
    /// acquire the connection — the caller keeps ownership.
    pub async fn new(
        pool: &PgPool,
        schema: SchemaGraph,
        options: ProfileOptions,
    ) -> Result<Self, ProfileError> {
        let mut conn = pool.acquire().await?;

        // Safety: a read-only role is strongly recommended for profiling.
        let is_superuser: String = sqlx::query_scalar("SELECT current_setting('is_superuser')")
            .fetch_one(&mut *conn)
            .await?;
        if is_superuser == "on" {
            warn!("connected as superuser; a read-only role is recommended for profiling");
            if options.strict_security {
                return Err(ProfileError::SuperuserNotAllowed);
            }
        }

        // Defense in depth on this dedicated connection.
        sqlx::query("SET default_transaction_read_only = on")
            .execute(&mut *conn)
            .await?;
        // statement_timeout takes an integer number of milliseconds. The value
        // comes from a u32 option, so this format is injection-free.
        let timeout_ms = u64::from(options.statement_timeout_secs) * 1000;
        sqlx::query(&format!("SET statement_timeout = {timeout_ms}"))
            .execute(&mut *conn)
            .await?;

        Ok(Self {
            conn,
            schema,
            options,
            audit_log: AuditLog::new(),
            skipped_sensitive: Vec::new(),
        })
    }

    /// Profile every table and column. Returns the complete [`DatabaseProfile`].
    pub async fn collect(&mut self) -> Result<DatabaseProfile, ProfileError> {
        if self.schema.tables.is_empty() {
            return Err(ProfileError::EmptySchema);
        }

        // Clone the schema so it can be read while `self` is borrowed mutably
        // to execute queries / append to the audit log.
        let schema = self.schema.clone();
        let mut tables = BTreeMap::new();
        for table in &schema.tables {
            let profile = self.profile_table(&schema, table).await?;
            tables.insert(table.name.clone(), profile);
        }

        let mut skipped = self.skipped_sensitive.clone();
        skipped.sort();
        skipped.dedup();

        Ok(DatabaseProfile {
            version: "1.0".to_string(),
            profiled_at: Utc::now().to_rfc3339(),
            source_hash: compute_source_hash(&schema),
            seedgen_version: env!("CARGO_PKG_VERSION").to_string(),
            options: ProfileOptionsSummary {
                cardinality_threshold: self.options.cardinality_threshold,
                skipped_sensitive: skipped,
            },
            tables,
        })
    }

    /// Return the full query plan without executing anything (dry-run mode).
    pub fn generate_queries(&self) -> Vec<PlannedQuery> {
        QueryBuilder::new(&self.schema, &self.options).build_all()
    }

    /// Emit the offline collection SQL: a single self-describing, read-only query
    /// whose result is a JSON document. Run it externally, then rebuild the
    /// profile with [`crate::profile::import_results`] — no connection needed.
    pub fn export_queries(&self) -> String {
        crate::profile::offline::export_collection_sql(&self.schema, &self.options)
    }

    /// Borrow the audit log (e.g. to inspect or render it).
    pub fn audit_log(&self) -> &AuditLog {
        &self.audit_log
    }

    /// Write the audit log to `path` (typically `.seedgen-profile-audit.log`).
    pub fn write_audit_log(&self, path: &Path) -> Result<(), ProfileError> {
        self.audit_log.write_to_file(path)
    }

    // --- internals --------------------------------------------------------

    async fn profile_table(
        &mut self,
        schema: &SchemaGraph,
        table: &Table,
    ) -> Result<TableProfile, ProfileError> {
        let rq = QueryBuilder::new(schema, &self.options).build_row_count(&table.name);
        let (row, dur) = self.run_one(&rq.sql).await?;
        let row_count = i64_or_zero(&row, "row_count")?.max(0) as u64;
        self.audit_log
            .record_query(&rq.sql, dur, format!("{row_count} rows"));

        // FK relationships where this table is the child → parent ratios.
        let fks: Vec<ForeignKey> = schema
            .foreign_keys
            .iter()
            .filter(|fk| fk.from_table == table.name)
            .cloned()
            .collect();
        let mut fk_columns: HashSet<String> = HashSet::new();
        let mut parent_ratios = BTreeMap::new();
        for fk in &fks {
            fk_columns.insert(fk.from_column.clone());
            let ratio = self.profile_parent_ratio(schema, fk).await?;
            // Keyed by parent table name, matching the YAML profile format.
            parent_ratios.insert(fk.to_table.clone(), ratio);
        }

        let mut columns = BTreeMap::new();
        for column in &table.columns {
            if fk_columns.contains(&column.name) {
                continue; // captured via parent ratios
            }
            if let Some(profile) = self.profile_column(schema, table, column).await? {
                columns.insert(column.name.clone(), profile);
            }
        }

        Ok(TableProfile {
            row_count,
            parent_ratios,
            columns,
        })
    }

    async fn profile_parent_ratio(
        &mut self,
        schema: &SchemaGraph,
        fk: &ForeignKey,
    ) -> Result<ParentRatio, ProfileError> {
        let rq = QueryBuilder::new(schema, &self.options).build_parent_ratio(
            &fk.to_table,
            &fk.from_table,
            &fk.from_column,
        );
        let (row, dur) = self.run_one(&rq.sql).await?;
        let avg = f64_or_zero(&row, "avg_ratio")?;
        let min = i64_or_zero(&row, "min_ratio")?.max(0) as u64;
        let max = i64_or_zero(&row, "max_ratio")?.max(0) as u64;
        let median = f64_or_zero(&row, "median_ratio")?;
        let stddev = f64_or_zero(&row, "stddev_ratio")?;
        let p25 = f64_or_zero(&row, "p25_ratio")?;
        let p75 = f64_or_zero(&row, "p75_ratio")?;
        let p95 = f64_or_zero(&row, "p95_ratio")?;
        let p99 = f64_or_zero(&row, "p99_ratio")?;
        self.audit_log
            .record_query(&rq.sql, dur, format!("avg={avg:.2}, p95={p95}"));

        let zq = QueryBuilder::new(schema, &self.options).build_parent_zero_count(
            &fk.to_table,
            &fk.to_column,
            &fk.from_table,
            &fk.from_column,
        );
        let (zrow, zdur) = self.run_one(&zq.sql).await?;
        let zero_count = i64_or_zero(&zrow, "zero_count")?.max(0) as u64;
        self.audit_log.record_query(
            &zq.sql,
            zdur,
            format!("{zero_count} parents with 0 children"),
        );

        // Parent row count → zero_rate.
        let pq = QueryBuilder::new(schema, &self.options).build_row_count(&fk.to_table);
        let (prow, pdur) = self.run_one(&pq.sql).await?;
        let parent_count = i64_or_zero(&prow, "row_count")?.max(0) as u64;
        self.audit_log
            .record_query(&pq.sql, pdur, format!("{parent_count} rows"));
        let zero_rate = if parent_count > 0 {
            Some(zero_count as f64 * 100.0 / parent_count as f64)
        } else {
            None
        };

        Ok(ParentRatio {
            column: fk.from_column.clone(),
            avg,
            min,
            max,
            median,
            stddev,
            percentiles: Some(Percentiles {
                p5: None,
                p10: None,
                p25,
                p50: median,
                p75,
                p90: None,
                p95,
                p99,
            }),
            zero_count: Some(zero_count),
            zero_rate,
        })
    }

    /// Profile one column. Returns `None` for columns that carry no profile
    /// (generated, or an unsupported type).
    async fn profile_column(
        &mut self,
        schema: &SchemaGraph,
        table: &Table,
        column: &Column,
    ) -> Result<Option<ColumnProfile>, ProfileError> {
        let t = table.name.clone();
        let c = column.name.clone();
        let subject = format!("\"{t}\".\"{c}\"");

        // --- skip rules (recorded in the audit log) ---
        if is_serial(column) {
            self.audit_log
                .record_skip(&subject, "serial / auto-increment");
            return Ok(Some(ColumnProfile::Serial));
        }
        if column.is_generated {
            self.audit_log.record_skip(&subject, "generated column");
            return Ok(None);
        }
        if is_excluded(&t, &c, &self.options.exclude_columns) {
            self.audit_log.record_skip(&subject, "excluded by user");
            return Ok(Some(ColumnProfile::SkippedExcluded));
        }
        if is_sensitive_column(&c) && !is_included(&t, &c, &self.options.include_columns) {
            let pattern = matched_sensitive_pattern(&c).unwrap_or("sensitive");
            let reason = format!("sensitive column (pattern: {pattern})");
            self.audit_log.record_skip(&subject, reason.clone());
            self.skipped_sensitive.push(format!("{t}.{c}"));
            return Ok(Some(ColumnProfile::SkippedSensitive { reason }));
        }

        let threshold = self.options.cardinality_threshold;
        let capture_monthly = self.options.capture_monthly;
        let capture_hourly = self.options.capture_hourly;
        let capture_percentiles = self.options.capture_percentiles;

        match column_category(&column.data_type) {
            Category::Boolean => {
                let q = QueryBuilder::new(schema, &self.options).build_boolean_stats(&t, &c);
                let (row, dur) = self.run_one(&q.sql).await?;
                let true_rate = f64_or_zero(&row, "true_rate")?;
                let null_rate = f64_or_zero(&row, "null_rate")?;
                self.audit_log
                    .record_query(&q.sql, dur, format!("true_rate={true_rate:.1}"));
                Ok(Some(ColumnProfile::Boolean {
                    true_rate,
                    null_rate,
                }))
            }
            Category::Numeric => {
                let q = QueryBuilder::new(schema, &self.options).build_numeric_stats(&t, &c);
                let (row, dur) = self.run_one(&q.sql).await?;
                let min = f64_or_zero(&row, "min_val")?;
                let max = f64_or_zero(&row, "max_val")?;
                let mean = f64_or_zero(&row, "mean_val")?;
                let stddev = f64_or_zero(&row, "stddev_val")?;
                let p25 = f64_or_zero(&row, "p25")?;
                let p50 = f64_or_zero(&row, "p50")?;
                let p75 = f64_or_zero(&row, "p75")?;
                let p95 = f64_or_zero(&row, "p95")?;
                let p99 = f64_or_zero(&row, "p99")?;
                let null_rate = f64_or_zero(&row, "null_rate")?;
                self.audit_log.record_query(
                    &q.sql,
                    dur,
                    format!("min={min}, max={max}, mean={mean:.2}"),
                );
                let percentiles = capture_percentiles.then_some(Percentiles {
                    p5: None,
                    p10: None,
                    p25,
                    p50,
                    p75,
                    p90: None,
                    p95,
                    p99,
                });
                Ok(Some(ColumnProfile::Numeric {
                    min,
                    max,
                    mean,
                    median: p50,
                    stddev,
                    null_rate,
                    percentiles,
                }))
            }
            Category::Timestamp => {
                let q = QueryBuilder::new(schema, &self.options).build_timestamp_stats(&t, &c);
                let (row, dur) = self.run_one(&q.sql).await?;
                let min_ts = opt_string(&row, "min_ts")?.unwrap_or_default();
                let max_ts = opt_string(&row, "max_ts")?.unwrap_or_default();
                let null_rate = f64_or_zero(&row, "null_rate")?;
                let weekday_ratio = opt_f64(&row, "weekday_ratio")?;
                self.audit_log
                    .record_query(&q.sql, dur, format!("range=[{min_ts}, {max_ts}]"));

                let mut monthly_density = BTreeMap::new();
                if capture_monthly {
                    let mq =
                        QueryBuilder::new(schema, &self.options).build_timestamp_monthly(&t, &c);
                    let (rows, mdur) = self.run_all(&mq.sql).await?;
                    for r in &rows {
                        let month = string_val(r, "month")?;
                        let cnt = i64_or_zero(r, "cnt")?.max(0) as u64;
                        monthly_density.insert(month, cnt);
                    }
                    self.audit_log.record_query(
                        &mq.sql,
                        mdur,
                        format!("{} months", monthly_density.len()),
                    );
                }

                let mut hourly_density = BTreeMap::new();
                if capture_hourly {
                    let hq =
                        QueryBuilder::new(schema, &self.options).build_timestamp_hourly(&t, &c);
                    let (rows, hdur) = self.run_all(&hq.sql).await?;
                    for r in &rows {
                        let hour = opt_i32(r, "hour")?.unwrap_or(0).clamp(0, 23) as u8;
                        let pct = f64_or_zero(r, "pct")?;
                        hourly_density.insert(hour, pct);
                    }
                    self.audit_log.record_query(
                        &hq.sql,
                        hdur,
                        format!("{} hours", hourly_density.len()),
                    );
                }

                Ok(Some(ColumnProfile::Timestamp {
                    range: (min_ts, max_ts),
                    null_rate,
                    weekday_ratio,
                    hourly_density,
                    monthly_density,
                }))
            }
            Category::String => {
                // Cardinality guard: capture individual values only when low.
                let cq = QueryBuilder::new(schema, &self.options).build_cardinality_check(&t, &c);
                let (crow, cdur) = self.run_one(&cq.sql).await?;
                let distinct = i64_or_zero(&crow, "distinct_count")?.max(0) as u64;
                let null_rate = f64_or_zero(&crow, "null_rate")?;
                self.audit_log
                    .record_query(&cq.sql, cdur, format!("{distinct} distinct"));

                if (distinct as usize) <= threshold {
                    Ok(Some(
                        self.collect_categorical(schema, &t, &c, null_rate).await?,
                    ))
                } else {
                    let sq = QueryBuilder::new(schema, &self.options).build_string_stats(&t, &c);
                    let (srow, sdur) = self.run_one(&sq.sql).await?;
                    let cardinality = i64_or_zero(&srow, "cardinality")?.max(0) as u64;
                    let avg_length = f64_or_zero(&srow, "avg_length")?;
                    let min_length = opt_i32(&srow, "min_length")?.map(|v| v.max(0) as u32);
                    let max_length = opt_i32(&srow, "max_length")?.map(|v| v.max(0) as u32);
                    let s_null = f64_or_zero(&srow, "null_rate")?;
                    self.audit_log.record_query(
                        &sq.sql,
                        sdur,
                        format!("cardinality={cardinality}"),
                    );
                    Ok(Some(ColumnProfile::StringStats {
                        semantic: None,
                        cardinality,
                        null_rate: s_null,
                        avg_length,
                        min_length,
                        max_length,
                    }))
                }
            }
            Category::Categorical => {
                // Enum: always categorical. Cardinality check supplies null_rate.
                let cq = QueryBuilder::new(schema, &self.options).build_cardinality_check(&t, &c);
                let (crow, cdur) = self.run_one(&cq.sql).await?;
                let distinct = i64_or_zero(&crow, "distinct_count")?.max(0) as u64;
                let null_rate = f64_or_zero(&crow, "null_rate")?;
                self.audit_log
                    .record_query(&cq.sql, cdur, format!("{distinct} distinct"));
                Ok(Some(
                    self.collect_categorical(schema, &t, &c, null_rate).await?,
                ))
            }
            Category::Skip => {
                self.audit_log
                    .record_skip(&subject, "unsupported type — not profiled");
                Ok(None)
            }
        }
    }

    async fn collect_categorical(
        &mut self,
        schema: &SchemaGraph,
        table: &str,
        column: &str,
        null_rate: f64,
    ) -> Result<ColumnProfile, ProfileError> {
        let dq =
            QueryBuilder::new(schema, &self.options).build_categorical_distribution(table, column);
        let (rows, dur) = self.run_all(&dq.sql).await?;
        let mut distribution = BTreeMap::new();
        for r in &rows {
            let value = string_val(r, "value")?;
            let pct = f64_or_zero(r, "pct")?;
            distribution.insert(value, pct);
        }
        self.audit_log
            .record_query(&dq.sql, dur, format!("{} categories", distribution.len()));
        Ok(ColumnProfile::Categorical {
            distribution,
            null_rate,
        })
    }

    async fn run_one(&mut self, sql: &str) -> Result<(PgRow, u64), ProfileError> {
        let start = Instant::now();
        let row = sqlx::query(sql).fetch_one(&mut *self.conn).await?;
        Ok((row, start.elapsed().as_millis() as u64))
    }

    async fn run_all(&mut self, sql: &str) -> Result<(Vec<PgRow>, u64), ProfileError> {
        let start = Instant::now();
        let rows = sqlx::query(sql).fetch_all(&mut *self.conn).await?;
        Ok((rows, start.elapsed().as_millis() as u64))
    }
}

/// Which sensitive pattern (if any) a column name matched — for the audit reason.
fn matched_sensitive_pattern(column_name: &str) -> Option<&'static str> {
    let lower = column_name.to_lowercase();
    SENSITIVE_PATTERNS
        .iter()
        .copied()
        .find(|p| lower.contains(p))
}

/// SHA-256 over table+column *names* only (no data), for stable profile identity.
pub(crate) fn compute_source_hash(schema: &SchemaGraph) -> String {
    use sha2::{Digest, Sha256};
    let mut parts: Vec<String> = schema
        .tables
        .iter()
        .map(|t| {
            let mut cols: Vec<&str> = t.columns.iter().map(|c| c.name.as_str()).collect();
            cols.sort_unstable();
            format!("{}:{}", t.name, cols.join(","))
        })
        .collect();
    parts.sort_unstable();
    let mut hasher = Sha256::new();
    hasher.update(parts.join(";").as_bytes());
    format!("sha256:{:x}", hasher.finalize())
}

// --- typed row accessors (tolerate SQL NULL via Option, propagate decode errors) ---

fn f64_or_zero(row: &PgRow, name: &str) -> Result<f64, ProfileError> {
    Ok(row.try_get::<Option<f64>, _>(name)?.unwrap_or(0.0))
}

fn opt_f64(row: &PgRow, name: &str) -> Result<Option<f64>, ProfileError> {
    Ok(row.try_get::<Option<f64>, _>(name)?)
}

fn i64_or_zero(row: &PgRow, name: &str) -> Result<i64, ProfileError> {
    Ok(row.try_get::<Option<i64>, _>(name)?.unwrap_or(0))
}

fn opt_i32(row: &PgRow, name: &str) -> Result<Option<i32>, ProfileError> {
    Ok(row.try_get::<Option<i32>, _>(name)?)
}

fn opt_string(row: &PgRow, name: &str) -> Result<Option<String>, ProfileError> {
    Ok(row.try_get::<Option<String>, _>(name)?)
}

fn string_val(row: &PgRow, name: &str) -> Result<String, ProfileError> {
    Ok(row.try_get::<String, _>(name)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::introspection::{Column, DataType, Table};

    fn col(name: &str) -> Column {
        Column {
            name: name.to_string(),
            data_type: DataType::Text,
            is_nullable: true,
            is_identity: false,
            is_generated: false,
            default_value: None,
            max_length: None,
            numeric_precision: None,
            numeric_scale: None,
        }
    }

    fn schema_with(tables: Vec<(&str, Vec<&str>)>) -> SchemaGraph {
        SchemaGraph {
            tables: tables
                .into_iter()
                .map(|(name, cols)| Table {
                    name: name.to_string(),
                    columns: cols.into_iter().map(col).collect(),
                    constraints: vec![],
                })
                .collect(),
            foreign_keys: vec![],
            enums: vec![],
        }
    }

    #[test]
    fn test_source_hash_is_deterministic_and_order_independent() {
        let a = schema_with(vec![("users", vec!["id", "email"]), ("orders", vec!["id"])]);
        // Same tables/columns, different declaration + column order.
        let b = schema_with(vec![("orders", vec!["id"]), ("users", vec!["email", "id"])]);
        assert_eq!(compute_source_hash(&a), compute_source_hash(&b));
        assert!(compute_source_hash(&a).starts_with("sha256:"));
    }

    #[test]
    fn test_source_hash_changes_with_schema() {
        let a = schema_with(vec![("users", vec!["id", "email"])]);
        let b = schema_with(vec![("users", vec!["id", "email", "phone"])]);
        assert_ne!(compute_source_hash(&a), compute_source_hash(&b));
    }

    #[test]
    fn test_matched_sensitive_pattern() {
        assert_eq!(matched_sensitive_pattern("password_hash"), Some("password"));
        assert_eq!(matched_sensitive_pattern("reset_token"), Some("token"));
        assert_eq!(matched_sensitive_pattern("email"), None);
    }
}
