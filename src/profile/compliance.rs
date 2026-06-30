//! Post-generation compliance: compare the generated data against the profile
//! it was generated from, and report drift per check.
//!
//! Failing checks are *reported, not fatal* — small samples drift naturally, and
//! not every captured statistic is reproduced by the generator yet (e.g. boolean
//! rates). The report is informational; the user can adjust scale or tolerance.

use std::collections::{BTreeMap, BTreeSet};

use sqlx::{PgPool, Row};

use crate::profile::errors::ProfileError;
use crate::profile::stats::{ColumnProfile, DatabaseProfile};

/// Default drift tolerance, in percentage points.
pub const DEFAULT_TOLERANCE: f64 = 5.0;

/// Validates generated data against a profile within a tolerance.
pub struct ComplianceValidator {
    profile: DatabaseProfile,
    tolerance: f64,
}

/// The outcome of running every applicable check.
#[derive(Debug, Clone)]
pub struct ComplianceReport {
    pub checks: Vec<ComplianceCheck>,
    pub passed: usize,
    pub failed: usize,
}

/// One comparison between a profiled statistic and the generated reality.
#[derive(Debug, Clone)]
pub enum ComplianceCheck {
    /// Categorical distribution drift (max per-category percentage-point gap).
    Distribution {
        table: String,
        column: String,
        max_drift: f64,
        passed: bool,
    },
    NullRate {
        table: String,
        column: String,
        expected: f64,
        actual: f64,
        passed: bool,
    },
    Ratio {
        child_table: String,
        parent_table: String,
        expected: f64,
        actual: f64,
        passed: bool,
    },
    BooleanRate {
        table: String,
        column: String,
        expected: f64,
        actual: f64,
        passed: bool,
    },
}

impl ComplianceCheck {
    pub fn passed(&self) -> bool {
        match self {
            ComplianceCheck::Distribution { passed, .. }
            | ComplianceCheck::NullRate { passed, .. }
            | ComplianceCheck::Ratio { passed, .. }
            | ComplianceCheck::BooleanRate { passed, .. } => *passed,
        }
    }

    fn describe(&self) -> String {
        match self {
            ComplianceCheck::Distribution {
                table,
                column,
                max_drift,
                ..
            } => format!("{table}.{column} distribution (max drift {max_drift:.1}%)"),
            ComplianceCheck::NullRate {
                table,
                column,
                expected,
                actual,
                ..
            } => format!("{table}.{column} null rate {actual:.1}% (target {expected:.1}%)"),
            ComplianceCheck::Ratio {
                child_table,
                parent_table,
                expected,
                actual,
                ..
            } => format!("{child_table}:{parent_table} ratio {actual:.2} (target {expected:.2})"),
            ComplianceCheck::BooleanRate {
                table,
                column,
                expected,
                actual,
                ..
            } => format!("{table}.{column} true rate {actual:.1}% (target {expected:.1}%)"),
        }
    }
}

impl ComplianceReport {
    pub fn all_passed(&self) -> bool {
        self.failed == 0
    }

    /// Print a `✓`/`✗` line per check.
    pub fn print(&self) {
        println!(
            "Profile compliance: {} passed, {} failed (tolerance applied)",
            self.passed, self.failed
        );
        for check in &self.checks {
            let mark = if check.passed() { "✓" } else { "✗" };
            println!("  {mark} {}", check.describe());
        }
    }
}

impl ComplianceValidator {
    pub fn new(profile: DatabaseProfile, tolerance: f64) -> Self {
        Self { profile, tolerance }
    }

    pub fn with_default_tolerance(profile: DatabaseProfile) -> Self {
        Self::new(profile, DEFAULT_TOLERANCE)
    }

    /// Run all applicable checks against the generated database.
    pub async fn validate(&self, pool: &PgPool) -> Result<ComplianceReport, ProfileError> {
        let mut checks = Vec::new();

        for (table, table_profile) in &self.profile.tables {
            // Table ratios (children per parent).
            for (parent, ratio) in &table_profile.parent_ratios {
                let child_count = count_rows(pool, table).await?;
                let parent_count = count_rows(pool, parent).await?;
                let actual = if parent_count > 0 {
                    child_count as f64 / parent_count as f64
                } else {
                    0.0
                };
                let passed = ratio_within(ratio.avg, actual, self.tolerance);
                checks.push(ComplianceCheck::Ratio {
                    child_table: table.clone(),
                    parent_table: parent.clone(),
                    expected: ratio.avg,
                    actual,
                    passed,
                });
            }

            // Per-column checks.
            for (column, profile) in &table_profile.columns {
                match profile {
                    ColumnProfile::Categorical {
                        distribution,
                        null_rate,
                    } => {
                        let actual = actual_distribution(pool, table, column).await?;
                        let max_drift = max_distribution_drift(distribution, &actual);
                        checks.push(ComplianceCheck::Distribution {
                            table: table.clone(),
                            column: column.clone(),
                            max_drift,
                            passed: max_drift <= self.tolerance,
                        });
                        checks.push(
                            self.null_rate_check(pool, table, column, *null_rate)
                                .await?,
                        );
                    }
                    ColumnProfile::Boolean {
                        true_rate,
                        null_rate,
                    } => {
                        let actual = actual_true_rate(pool, table, column).await?;
                        checks.push(ComplianceCheck::BooleanRate {
                            table: table.clone(),
                            column: column.clone(),
                            expected: *true_rate,
                            actual,
                            passed: (actual - true_rate).abs() <= self.tolerance,
                        });
                        checks.push(
                            self.null_rate_check(pool, table, column, *null_rate)
                                .await?,
                        );
                    }
                    ColumnProfile::Numeric { null_rate, .. }
                    | ColumnProfile::StringStats { null_rate, .. }
                    | ColumnProfile::Timestamp { null_rate, .. } => {
                        checks.push(
                            self.null_rate_check(pool, table, column, *null_rate)
                                .await?,
                        );
                    }
                    ColumnProfile::Serial
                    | ColumnProfile::SkippedSensitive { .. }
                    | ColumnProfile::SkippedExcluded => {}
                }
            }
        }

        let passed = checks.iter().filter(|c| c.passed()).count();
        let failed = checks.len() - passed;
        Ok(ComplianceReport {
            checks,
            passed,
            failed,
        })
    }

    async fn null_rate_check(
        &self,
        pool: &PgPool,
        table: &str,
        column: &str,
        expected: f64,
    ) -> Result<ComplianceCheck, ProfileError> {
        let actual = actual_null_rate(pool, table, column).await?;
        Ok(ComplianceCheck::NullRate {
            table: table.to_string(),
            column: column.to_string(),
            expected,
            actual,
            passed: (actual - expected).abs() <= self.tolerance,
        })
    }
}

/// Maximum per-category percentage-point gap over the union of the two maps.
pub fn max_distribution_drift(
    expected: &BTreeMap<String, f64>,
    actual: &BTreeMap<String, f64>,
) -> f64 {
    let keys: BTreeSet<&String> = expected.keys().chain(actual.keys()).collect();
    keys.into_iter()
        .map(|k| {
            let e = expected.get(k).copied().unwrap_or(0.0);
            let a = actual.get(k).copied().unwrap_or(0.0);
            (e - a).abs()
        })
        .fold(0.0, f64::max)
}

/// Ratios are compared with a *relative* tolerance (they are not percentages).
fn ratio_within(expected: f64, actual: f64, tolerance: f64) -> bool {
    if expected.abs() < f64::EPSILON {
        return actual.abs() < f64::EPSILON;
    }
    (actual - expected).abs() / expected.abs() * 100.0 <= tolerance
}

fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

async fn count_rows(pool: &PgPool, table: &str) -> Result<i64, ProfileError> {
    let sql = format!("SELECT COUNT(*) AS n FROM {}", quote_ident(table));
    let row = sqlx::query(&sql).fetch_one(pool).await?;
    Ok(row.try_get::<i64, _>("n")?)
}

async fn actual_null_rate(pool: &PgPool, table: &str, column: &str) -> Result<f64, ProfileError> {
    let sql = format!(
        "SELECT (COUNT(*) FILTER (WHERE {c} IS NULL) * 100.0 / NULLIF(COUNT(*), 0))::float8 AS r \
         FROM {t}",
        c = quote_ident(column),
        t = quote_ident(table),
    );
    let row = sqlx::query(&sql).fetch_one(pool).await?;
    Ok(row.try_get::<Option<f64>, _>("r")?.unwrap_or(0.0))
}

async fn actual_true_rate(pool: &PgPool, table: &str, column: &str) -> Result<f64, ProfileError> {
    let sql = format!(
        "SELECT (COUNT(*) FILTER (WHERE {c} = TRUE) * 100.0 / NULLIF(COUNT(*), 0))::float8 AS r \
         FROM {t}",
        c = quote_ident(column),
        t = quote_ident(table),
    );
    let row = sqlx::query(&sql).fetch_one(pool).await?;
    Ok(row.try_get::<Option<f64>, _>("r")?.unwrap_or(0.0))
}

async fn actual_distribution(
    pool: &PgPool,
    table: &str,
    column: &str,
) -> Result<BTreeMap<String, f64>, ProfileError> {
    let sql = format!(
        "SELECT ({c})::text AS value, \
         (COUNT(*) * 100.0 / SUM(COUNT(*)) OVER ())::float8 AS pct \
         FROM {t} WHERE {c} IS NOT NULL GROUP BY {c}",
        c = quote_ident(column),
        t = quote_ident(table),
    );
    let rows = sqlx::query(&sql).fetch_all(pool).await?;
    let mut dist = BTreeMap::new();
    for row in &rows {
        let value: String = row.try_get("value")?;
        let pct: f64 = row.try_get::<Option<f64>, _>("pct")?.unwrap_or(0.0);
        dist.insert(value, pct);
    }
    Ok(dist)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_max_distribution_drift_simple() {
        let expected = BTreeMap::from([("a".into(), 60.0), ("b".into(), 40.0)]);
        let actual = BTreeMap::from([("a".into(), 58.0), ("b".into(), 42.0)]);
        assert!((max_distribution_drift(&expected, &actual) - 2.0).abs() < 1e-9);
    }

    #[test]
    fn test_max_distribution_drift_missing_key() {
        // `b` present in expected but absent from actual → full 40-point drift.
        let expected = BTreeMap::from([("a".into(), 60.0), ("b".into(), 40.0)]);
        let actual = BTreeMap::from([("a".into(), 100.0)]);
        assert!((max_distribution_drift(&expected, &actual) - 40.0).abs() < 1e-9);
    }

    #[test]
    fn test_max_distribution_drift_identical_is_zero() {
        let d = BTreeMap::from([("a".into(), 50.0), ("b".into(), 50.0)]);
        assert_eq!(max_distribution_drift(&d, &d), 0.0);
    }

    #[test]
    fn test_ratio_within_relative_tolerance() {
        assert!(ratio_within(5.0, 5.0, 5.0));
        assert!(ratio_within(5.0, 5.2, 5.0)); // 4% drift, within 5%
        assert!(!ratio_within(5.0, 5.5, 5.0)); // 10% drift, exceeds 5%
        assert!(ratio_within(0.0, 0.0, 5.0));
        assert!(!ratio_within(0.0, 1.0, 5.0));
    }

    #[test]
    fn test_report_counts_and_all_passed() {
        let report = ComplianceReport {
            checks: vec![
                ComplianceCheck::Distribution {
                    table: "t".into(),
                    column: "c".into(),
                    max_drift: 1.0,
                    passed: true,
                },
                ComplianceCheck::BooleanRate {
                    table: "t".into(),
                    column: "b".into(),
                    expected: 75.0,
                    actual: 50.0,
                    passed: false,
                },
            ],
            passed: 1,
            failed: 1,
        };
        assert!(!report.all_passed());
        assert_eq!(report.checks.iter().filter(|c| c.passed()).count(), 1);
    }
}
