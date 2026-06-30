//! Type-safe builders for the read-only, aggregate-only SQL that profiling runs.
//!
//! # Security model (Layer 1: query whitelist)
//!
//! Every query is assembled from a fixed template. The *only* values ever
//! interpolated are table/column identifiers, and those go through
//! [`quote_ident`] (double-quoted, internal quotes escaped) — never a data
//! value, never a `WHERE col = <value>` predicate. As a result every query
//! produced here:
//!
//! * starts with `SELECT`,
//! * is a pure aggregate (`COUNT`/`MIN`/`MAX`/`AVG`/`STDDEV`/`PERCENTILE_CONT`,
//!   or a low-cardinality `GROUP BY`), and
//! * never contains `SELECT *`, `INSERT`, `UPDATE`, `DELETE`, or any DDL.
//!
//! Percentile fractions (`0.25`, `0.5`, …) are template constants, not user
//! input. The cardinality threshold is *not* interpolated into SQL — the guard
//! compares the returned `distinct_count` in Rust.

use std::collections::HashSet;

use crate::introspection::{Column, DataType, SchemaGraph, Table};
use crate::profile::config::ProfileOptions;
use crate::profile::sensitive::{is_excluded, is_included, is_sensitive_column};

/// What a [`PlannedQuery`] computes — lets the collector route each result row
/// back to the right place without re-parsing SQL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryKind {
    RowCount,
    NumericStats,
    CardinalityCheck,
    CategoricalDistribution,
    BooleanStats,
    StringStats,
    TimestampStats,
    TimestampMonthly,
    TimestampHourly,
    ParentRatio,
    ParentZeroCount,
}

/// A single planned, read-only query plus the metadata needed to interpret it.
#[derive(Debug, Clone)]
pub struct PlannedQuery {
    pub kind: QueryKind,
    /// Table the query targets. For parent-ratio queries this is the CHILD table.
    pub table: String,
    /// Column the query targets, if any. For parent-ratio queries this is the
    /// FK column on the child table.
    pub column: Option<String>,
    /// For parent-ratio / zero-count queries, the parent table; otherwise `None`.
    pub parent_table: Option<String>,
    /// The SQL text. Identifiers are quoted; no data values are interpolated.
    pub sql: String,
}

/// Builds the full set of profiling queries for a schema, honoring the skip
/// rules (sensitive / excluded / serial / generated / FK) and capture options.
pub struct QueryBuilder<'a> {
    schema: &'a SchemaGraph,
    options: &'a ProfileOptions,
}

impl<'a> QueryBuilder<'a> {
    pub fn new(schema: &'a SchemaGraph, options: &'a ProfileOptions) -> Self {
        Self { schema, options }
    }

    /// Generate every query for every table and profileable column.
    pub fn build_all(&self) -> Vec<PlannedQuery> {
        let mut queries = Vec::new();

        for table in &self.schema.tables {
            queries.push(self.build_row_count(&table.name));

            // FK relationships where this table is the child → parent ratios.
            let mut fk_columns: HashSet<&str> = HashSet::new();
            for fk in self
                .schema
                .foreign_keys
                .iter()
                .filter(|fk| fk.from_table == table.name)
            {
                fk_columns.insert(fk.from_column.as_str());
                queries.push(self.build_parent_ratio(
                    &fk.to_table,
                    &fk.from_table,
                    &fk.from_column,
                ));
                queries.push(self.build_parent_zero_count(
                    &fk.to_table,
                    &fk.to_column,
                    &fk.from_table,
                    &fk.from_column,
                ));
            }

            for column in &table.columns {
                // FK columns are captured via parent ratios, not value profiling.
                if fk_columns.contains(column.name.as_str()) {
                    continue;
                }
                queries.extend(self.plan_column(table, column));
            }
        }

        queries
    }

    /// Decide which (if any) queries to emit for one column.
    fn plan_column(&self, table: &Table, column: &Column) -> Vec<PlannedQuery> {
        let t = &table.name;
        let c = &column.name;

        // Nothing to profile: auto-increment, generated, or explicitly excluded.
        if is_serial(column)
            || column.is_generated
            || is_excluded(t, c, &self.options.exclude_columns)
        {
            return Vec::new();
        }
        // Sensitive columns are skipped unless explicitly opted back in.
        if is_sensitive_column(c) && !is_included(t, c, &self.options.include_columns) {
            return Vec::new();
        }

        match column_category(&column.data_type) {
            Category::Boolean => vec![self.build_boolean_stats(t, c)],
            Category::Numeric => vec![self.build_numeric_stats(t, c)],
            Category::Timestamp => {
                let mut v = vec![self.build_timestamp_stats(t, c)];
                if self.options.capture_monthly {
                    v.push(self.build_timestamp_monthly(t, c));
                }
                if self.options.capture_hourly {
                    v.push(self.build_timestamp_hourly(t, c));
                }
                v
            }
            // Strings: cardinality decides categorical-vs-string at execution
            // time, so plan all three; the collector runs the guard and picks.
            Category::String => vec![
                self.build_cardinality_check(t, c),
                self.build_categorical_distribution(t, c),
                self.build_string_stats(t, c),
            ],
            // Enums are categorical by definition — no string-length stats.
            Category::Categorical => vec![
                self.build_cardinality_check(t, c),
                self.build_categorical_distribution(t, c),
            ],
            Category::Skip => Vec::new(),
        }
    }

    // --- Table-level ------------------------------------------------------

    /// `SELECT COUNT(*) ...` — total rows in a table.
    pub fn build_row_count(&self, table: &str) -> PlannedQuery {
        let sql = format!("SELECT COUNT(*) AS row_count FROM {}", quote_ident(table));
        table_query(QueryKind::RowCount, table, sql)
    }

    /// Children-per-parent statistics (avg / min / max / median / stddev / p95 / p99).
    pub fn build_parent_ratio(
        &self,
        parent_table: &str,
        child_table: &str,
        fk_column: &str,
    ) -> PlannedQuery {
        let child = quote_ident(child_table);
        let fk = quote_ident(fk_column);
        let sql = format!(
            "SELECT \
             AVG(cnt::float8) AS avg_ratio, \
             MIN(cnt) AS min_ratio, \
             MAX(cnt) AS max_ratio, \
             PERCENTILE_CONT(0.25) WITHIN GROUP (ORDER BY cnt::float8) AS p25_ratio, \
             PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY cnt::float8) AS median_ratio, \
             PERCENTILE_CONT(0.75) WITHIN GROUP (ORDER BY cnt::float8) AS p75_ratio, \
             STDDEV(cnt::float8) AS stddev_ratio, \
             PERCENTILE_CONT(0.95) WITHIN GROUP (ORDER BY cnt::float8) AS p95_ratio, \
             PERCENTILE_CONT(0.99) WITHIN GROUP (ORDER BY cnt::float8) AS p99_ratio \
             FROM (SELECT {fk}, COUNT(*) AS cnt FROM {child} GROUP BY {fk}) sub"
        );
        PlannedQuery {
            kind: QueryKind::ParentRatio,
            table: child_table.to_string(),
            column: Some(fk_column.to_string()),
            parent_table: Some(parent_table.to_string()),
            sql,
        }
    }

    /// Number of parent rows that have zero children.
    pub fn build_parent_zero_count(
        &self,
        parent_table: &str,
        parent_pk: &str,
        child_table: &str,
        fk_column: &str,
    ) -> PlannedQuery {
        let parent = quote_ident(parent_table);
        let pk = quote_ident(parent_pk);
        let child = quote_ident(child_table);
        let fk = quote_ident(fk_column);
        let sql = format!(
            "SELECT COUNT(*) AS zero_count \
             FROM {parent} p \
             LEFT JOIN {child} c ON c.{fk} = p.{pk} \
             WHERE c.{fk} IS NULL"
        );
        PlannedQuery {
            kind: QueryKind::ParentZeroCount,
            table: child_table.to_string(),
            column: Some(fk_column.to_string()),
            parent_table: Some(parent_table.to_string()),
            sql,
        }
    }

    // --- Column-level -----------------------------------------------------

    /// Numeric distribution: min / max / mean / stddev / percentiles / null rate.
    pub fn build_numeric_stats(&self, table: &str, column: &str) -> PlannedQuery {
        let t = quote_ident(table);
        let c = quote_ident(column);
        let sql = format!(
            "SELECT \
             MIN(({c})::float8) AS min_val, \
             MAX(({c})::float8) AS max_val, \
             AVG(({c})::float8) AS mean_val, \
             STDDEV(({c})::float8) AS stddev_val, \
             PERCENTILE_CONT(0.25) WITHIN GROUP (ORDER BY ({c})::float8) AS p25, \
             PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY ({c})::float8) AS p50, \
             PERCENTILE_CONT(0.75) WITHIN GROUP (ORDER BY ({c})::float8) AS p75, \
             PERCENTILE_CONT(0.95) WITHIN GROUP (ORDER BY ({c})::float8) AS p95, \
             PERCENTILE_CONT(0.99) WITHIN GROUP (ORDER BY ({c})::float8) AS p99, \
             (COUNT(*) FILTER (WHERE {c} IS NULL) * 100.0 / NULLIF(COUNT(*), 0))::float8 AS null_rate \
             FROM {t}"
        );
        column_query(QueryKind::NumericStats, table, column, sql)
    }

    /// Distinct-value count plus null rate — drives the cardinality guard.
    pub fn build_cardinality_check(&self, table: &str, column: &str) -> PlannedQuery {
        let t = quote_ident(table);
        let c = quote_ident(column);
        let sql = format!(
            "SELECT \
             COUNT(DISTINCT {c}) AS distinct_count, \
             (COUNT(*) FILTER (WHERE {c} IS NULL) * 100.0 / NULLIF(COUNT(*), 0))::float8 AS null_rate \
             FROM {t}"
        );
        column_query(QueryKind::CardinalityCheck, table, column, sql)
    }

    /// Value → percentage distribution. Only executed for low-cardinality columns.
    pub fn build_categorical_distribution(&self, table: &str, column: &str) -> PlannedQuery {
        let t = quote_ident(table);
        let c = quote_ident(column);
        let sql = format!(
            "SELECT ({c})::text AS value, (COUNT(*) * 100.0 / SUM(COUNT(*)) OVER ())::float8 AS pct \
             FROM {t} WHERE {c} IS NOT NULL GROUP BY {c} ORDER BY pct DESC"
        );
        column_query(QueryKind::CategoricalDistribution, table, column, sql)
    }

    /// Boolean true rate and null rate.
    pub fn build_boolean_stats(&self, table: &str, column: &str) -> PlannedQuery {
        let t = quote_ident(table);
        let c = quote_ident(column);
        let sql = format!(
            "SELECT \
             (COUNT(*) FILTER (WHERE {c} = TRUE) * 100.0 / NULLIF(COUNT(*), 0))::float8 AS true_rate, \
             (COUNT(*) FILTER (WHERE {c} IS NULL) * 100.0 / NULLIF(COUNT(*), 0))::float8 AS null_rate \
             FROM {t}"
        );
        column_query(QueryKind::BooleanStats, table, column, sql)
    }

    /// High-cardinality string aggregates — lengths only, never actual values.
    pub fn build_string_stats(&self, table: &str, column: &str) -> PlannedQuery {
        let t = quote_ident(table);
        let c = quote_ident(column);
        let sql = format!(
            "SELECT \
             COUNT(DISTINCT {c}) AS cardinality, \
             AVG(LENGTH({c}))::float8 AS avg_length, \
             MIN(LENGTH({c})) AS min_length, \
             MAX(LENGTH({c})) AS max_length, \
             (COUNT(*) FILTER (WHERE {c} IS NULL) * 100.0 / NULLIF(COUNT(*), 0))::float8 AS null_rate \
             FROM {t}"
        );
        column_query(QueryKind::StringStats, table, column, sql)
    }

    /// Timestamp range, null rate, and weekday ratio in one pass.
    pub fn build_timestamp_stats(&self, table: &str, column: &str) -> PlannedQuery {
        let t = quote_ident(table);
        let c = quote_ident(column);
        let sql = format!(
            "SELECT \
             MIN({c})::text AS min_ts, \
             MAX({c})::text AS max_ts, \
             (COUNT(*) FILTER (WHERE {c} IS NULL) * 100.0 / NULLIF(COUNT(*), 0))::float8 AS null_rate, \
             (COUNT(*) FILTER (WHERE EXTRACT(DOW FROM {c}) BETWEEN 1 AND 5) * 100.0 \
             / NULLIF(COUNT(*) FILTER (WHERE {c} IS NOT NULL), 0))::float8 AS weekday_ratio \
             FROM {t}"
        );
        column_query(QueryKind::TimestampStats, table, column, sql)
    }

    /// Per-month row counts (`YYYY-MM` → count).
    pub fn build_timestamp_monthly(&self, table: &str, column: &str) -> PlannedQuery {
        let t = quote_ident(table);
        let c = quote_ident(column);
        let sql = format!(
            "SELECT TO_CHAR(DATE_TRUNC('month', {c}), 'YYYY-MM') AS month, COUNT(*) AS cnt \
             FROM {t} WHERE {c} IS NOT NULL GROUP BY 1 ORDER BY 1"
        );
        column_query(QueryKind::TimestampMonthly, table, column, sql)
    }

    /// Per-hour distribution (hour `0..23` → percentage).
    pub fn build_timestamp_hourly(&self, table: &str, column: &str) -> PlannedQuery {
        let t = quote_ident(table);
        let c = quote_ident(column);
        let sql = format!(
            "SELECT EXTRACT(HOUR FROM {c})::int AS hour, \
             (COUNT(*) * 100.0 / SUM(COUNT(*)) OVER ())::float8 AS pct \
             FROM {t} WHERE {c} IS NOT NULL GROUP BY 1 ORDER BY 1"
        );
        column_query(QueryKind::TimestampHourly, table, column, sql)
    }
}

/// Coarse profiling category for a column's SQL type.
pub(crate) enum Category {
    Boolean,
    Numeric,
    Timestamp,
    String,
    Categorical,
    Skip,
}

pub(crate) fn column_category(dt: &DataType) -> Category {
    match dt {
        DataType::Boolean => Category::Boolean,
        DataType::SmallInt
        | DataType::Integer
        | DataType::BigInt
        | DataType::Real
        | DataType::DoublePrecision
        | DataType::Numeric => Category::Numeric,
        DataType::Date | DataType::Timestamp | DataType::TimestampTz => Category::Timestamp,
        DataType::Char | DataType::Varchar | DataType::Text => Category::String,
        DataType::Enum(_) => Category::Categorical,
        // Not profiled (no meaningful aggregate, identifier/binary/structured, or
        // a cast we defer): Money, Bytea, Time, TimeTz, Interval, Uuid, Json,
        // Jsonb, Inet, Cidr, MacAddr, Array, Other.
        _ => Category::Skip,
    }
}

/// Whether a column is auto-assigned (identity or `nextval(...)` default), in
/// which case there is nothing to profile. Mirrors the generation engine.
pub(crate) fn is_serial(column: &Column) -> bool {
    column.is_identity
        || column
            .default_value
            .as_deref()
            .map(|d| d.contains("nextval"))
            .unwrap_or(false)
}

/// Double-quote an identifier, escaping any embedded double quotes. This is the
/// only place identifiers enter SQL — values are never interpolated.
fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn table_query(kind: QueryKind, table: &str, sql: String) -> PlannedQuery {
    PlannedQuery {
        kind,
        table: table.to_string(),
        column: None,
        parent_table: None,
        sql,
    }
}

fn column_query(kind: QueryKind, table: &str, column: &str, sql: String) -> PlannedQuery {
    PlannedQuery {
        kind,
        table: table.to_string(),
        column: Some(column.to_string()),
        parent_table: None,
        sql,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::introspection::{Column, DataType, EnumType, ForeignKey, SchemaGraph, Table};

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

    fn serial(name: &str) -> Column {
        Column {
            is_identity: true,
            is_nullable: false,
            ..col(name, DataType::Integer)
        }
    }

    /// users (root) + orders (child of users). 6 + 5 columns; one FK.
    fn fixture() -> SchemaGraph {
        let users = Table {
            name: "users".to_string(),
            columns: vec![
                serial("id"),
                col("email", DataType::Text),
                col("role", DataType::Enum("user_role".to_string())),
                col("is_active", DataType::Boolean),
                col("created_at", DataType::Timestamp),
                col("password_hash", DataType::Text),
            ],
            constraints: vec![],
        };
        let orders = Table {
            name: "orders".to_string(),
            columns: vec![
                serial("id"),
                col("user_id", DataType::Integer),
                col("status", DataType::Text),
                col("total", DataType::Numeric),
                col("created_at", DataType::Timestamp),
            ],
            constraints: vec![],
        };
        SchemaGraph {
            tables: vec![users, orders],
            foreign_keys: vec![ForeignKey {
                from_table: "orders".to_string(),
                from_column: "user_id".to_string(),
                to_table: "users".to_string(),
                to_column: "id".to_string(),
                is_nullable: false,
                is_deferrable: false,
            }],
            enums: vec![EnumType {
                name: "user_role".to_string(),
                values: vec!["user".to_string(), "admin".to_string()],
            }],
        }
    }

    fn count_kind(queries: &[PlannedQuery], kind: QueryKind) -> usize {
        queries.iter().filter(|q| q.kind == kind).count()
    }

    /// True if any column-level (non-parent) query targets `table.column`.
    fn has_column_query(queries: &[PlannedQuery], table: &str, column: &str) -> bool {
        queries.iter().any(|q| {
            !matches!(q.kind, QueryKind::ParentRatio | QueryKind::ParentZeroCount)
                && q.table == table
                && q.column.as_deref() == Some(column)
        })
    }

    #[test]
    fn test_build_all_query_count_matches_expected() {
        let schema = fixture();
        let options = ProfileOptions::default();
        let queries = QueryBuilder::new(&schema, &options).build_all();

        // users: row_count(1) + email String(3) + role Enum(2) + is_active Bool(1)
        //        + created_at Timestamp(3)  [id serial & password_hash sensitive skipped] = 10
        // orders: row_count(1) + FK ratio+zero(2) + status String(3) + total Numeric(1)
        //        + created_at Timestamp(3)  [id serial & user_id FK skipped] = 10
        assert_eq!(queries.len(), 20);
        assert_eq!(count_kind(&queries, QueryKind::RowCount), 2);
        assert_eq!(count_kind(&queries, QueryKind::ParentRatio), 1);
        assert_eq!(count_kind(&queries, QueryKind::ParentZeroCount), 1);
        assert_eq!(count_kind(&queries, QueryKind::TimestampMonthly), 2);
        assert_eq!(count_kind(&queries, QueryKind::TimestampHourly), 2);
    }

    #[test]
    fn test_query_builder_all_queries_are_select() {
        let schema = fixture();
        let options = ProfileOptions::default();
        for q in QueryBuilder::new(&schema, &options).build_all() {
            assert!(
                q.sql.trim_start().to_uppercase().starts_with("SELECT"),
                "query did not start with SELECT: {}",
                q.sql
            );
        }
    }

    #[test]
    fn test_query_builder_never_produces_select_star() {
        let schema = fixture();
        let options = ProfileOptions::default();
        for q in QueryBuilder::new(&schema, &options).build_all() {
            assert!(!q.sql.contains("SELECT *"), "SELECT * found in: {}", q.sql);
        }
    }

    #[test]
    fn test_no_query_contains_dml_keywords() {
        let schema = fixture();
        let options = ProfileOptions::default();
        // SQL keywords are uppercase in our templates; introspected identifiers
        // are echoed as-is. The fixture uses lowercase identifiers, so a
        // case-sensitive keyword check has no identifier false positives.
        for q in QueryBuilder::new(&schema, &options).build_all() {
            for kw in ["INSERT", "UPDATE", "DELETE", "DROP", "TRUNCATE", "ALTER"] {
                assert!(!q.sql.contains(kw), "`{kw}` found in: {}", q.sql);
            }
        }
    }

    #[test]
    fn test_serial_sensitive_and_fk_columns_are_skipped() {
        let schema = fixture();
        let options = ProfileOptions::default();
        let queries = QueryBuilder::new(&schema, &options).build_all();

        // Serial PKs, the sensitive column, and the FK column get no column queries.
        assert!(!has_column_query(&queries, "users", "id"));
        assert!(!has_column_query(&queries, "users", "password_hash"));
        assert!(!has_column_query(&queries, "orders", "id"));
        assert!(!has_column_query(&queries, "orders", "user_id"));
        assert!(!queries.iter().any(|q| q.sql.contains("password_hash")));

        // Regular columns are profiled.
        assert!(has_column_query(&queries, "users", "email"));
        assert!(has_column_query(&queries, "users", "role"));
        assert!(has_column_query(&queries, "orders", "total"));
    }

    #[test]
    fn test_include_override_profiles_sensitive_column() {
        let schema = fixture();
        let options = ProfileOptions {
            include_columns: vec!["users.password_hash".to_string()],
            ..ProfileOptions::default()
        };
        let queries = QueryBuilder::new(&schema, &options).build_all();
        // Now profiled as a string column: cardinality + distribution + string_stats.
        assert!(has_column_query(&queries, "users", "password_hash"));
        assert_eq!(queries.len(), 23);
    }

    #[test]
    fn test_exclude_skips_column() {
        let schema = fixture();
        let options = ProfileOptions {
            exclude_columns: vec!["users.email".to_string()],
            ..ProfileOptions::default()
        };
        let queries = QueryBuilder::new(&schema, &options).build_all();
        assert!(!has_column_query(&queries, "users", "email"));
        // email was a String column (3 queries) → 20 - 3 = 17.
        assert_eq!(queries.len(), 17);
    }

    #[test]
    fn test_capture_flags_gate_timestamp_density() {
        let schema = fixture();
        let options = ProfileOptions {
            capture_hourly: false,
            capture_monthly: false,
            ..ProfileOptions::default()
        };
        let queries = QueryBuilder::new(&schema, &options).build_all();
        assert_eq!(count_kind(&queries, QueryKind::TimestampMonthly), 0);
        assert_eq!(count_kind(&queries, QueryKind::TimestampHourly), 0);
        // Two timestamp columns lose 2 queries each → 20 - 4 = 16.
        assert_eq!(queries.len(), 16);
    }

    #[test]
    fn test_parent_ratio_targets_child_and_parent() {
        let schema = fixture();
        let options = ProfileOptions::default();
        let queries = QueryBuilder::new(&schema, &options).build_all();
        let ratio = queries
            .iter()
            .find(|q| q.kind == QueryKind::ParentRatio)
            .expect("a parent ratio query");
        assert_eq!(ratio.table, "orders");
        assert_eq!(ratio.column.as_deref(), Some("user_id"));
        assert_eq!(ratio.parent_table.as_deref(), Some("users"));
        assert!(ratio.sql.contains("FROM \"orders\""));
        assert!(ratio.sql.contains("GROUP BY \"user_id\""));
    }

    #[test]
    fn test_row_count_sql_is_exact() {
        let schema = fixture();
        let options = ProfileOptions::default();
        let qb = QueryBuilder::new(&schema, &options);
        assert_eq!(
            qb.build_row_count("users").sql,
            "SELECT COUNT(*) AS row_count FROM \"users\""
        );
    }

    #[test]
    fn test_quote_ident_escapes_internal_quotes() {
        assert_eq!(quote_ident("users"), "\"users\"");
        assert_eq!(quote_ident("we\"ird"), "\"we\"\"ird\"");
    }
}
