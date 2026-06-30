//! Serializable statistical profile of a database.
//!
//! Every type here derives `Serialize + Deserialize` and uses [`BTreeMap`] so
//! that YAML/JSON output is deterministic (sorted keys). No type here holds
//! row-level data — only aggregate statistics.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::profile::config::ProfileOptionsSummary;

/// Complete statistical profile for one database.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DatabaseProfile {
    pub version: String,
    pub profiled_at: String,
    pub source_hash: String,
    pub seedgen_version: String,
    pub options: ProfileOptionsSummary,
    pub tables: BTreeMap<String, TableProfile>,
}

/// Profile for one table: row count, optional FK ratios, and per-column stats.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TableProfile {
    pub row_count: u64,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub parent_ratios: BTreeMap<String, ParentRatio>,
    pub columns: BTreeMap<String, ColumnProfile>,
}

/// Statistics about a children-per-parent (FK) relationship.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParentRatio {
    pub column: String,
    pub avg: f64,
    pub min: u64,
    pub max: u64,
    pub median: f64,
    pub stddev: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub percentiles: Option<Percentiles>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zero_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zero_rate: Option<f64>,
}

/// Percentile breakdown. `p25/p50/p75/p95/p99` are always present; the rest are
/// optional (captured only when requested).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Percentiles {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p5: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p10: Option<f64>,
    pub p25: f64,
    pub p50: f64,
    pub p75: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p90: Option<f64>,
    pub p95: f64,
    pub p99: f64,
}

/// Column-level statistics. The shape varies by the column's detected kind;
/// the `type` field is the YAML/JSON discriminator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ColumnProfile {
    /// Auto-increment / serial — nothing to profile.
    #[serde(rename = "serial")]
    Serial,

    /// Skipped because the column name matched a sensitive pattern.
    #[serde(rename = "skipped_sensitive")]
    SkippedSensitive { reason: String },

    /// Explicitly excluded by the user.
    #[serde(rename = "skipped_excluded")]
    SkippedExcluded,

    /// Low-cardinality string/enum — exact value distribution captured.
    #[serde(rename = "categorical")]
    Categorical {
        distribution: BTreeMap<String, f64>,
        null_rate: f64,
    },

    /// High-cardinality string — only aggregate stats, NEVER actual values.
    #[serde(rename = "string")]
    StringStats {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        semantic: Option<String>,
        cardinality: u64,
        null_rate: f64,
        avg_length: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min_length: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_length: Option<u32>,
    },

    /// Numeric column — statistical distribution.
    #[serde(rename = "numeric")]
    Numeric {
        min: f64,
        max: f64,
        mean: f64,
        median: f64,
        stddev: f64,
        null_rate: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        percentiles: Option<Percentiles>,
    },

    /// Boolean column.
    #[serde(rename = "boolean")]
    Boolean { true_rate: f64, null_rate: f64 },

    /// Timestamp column — range plus optional temporal patterns.
    #[serde(rename = "timestamp")]
    Timestamp {
        range: (String, String),
        null_rate: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        weekday_ratio: Option<f64>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        hourly_density: BTreeMap<u8, f64>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        monthly_density: BTreeMap<String, u64>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_percentiles() -> Percentiles {
        Percentiles {
            p5: Some(4.99),
            p10: Some(9.99),
            p25: 19.99,
            p50: 42.15,
            p75: 79.99,
            p90: Some(149.99),
            p95: 249.99,
            p99: 599.99,
        }
    }

    fn yaml_roundtrip(profile: &ColumnProfile) {
        let yaml = serde_yaml::to_string(profile).expect("serialize");
        let parsed: ColumnProfile = serde_yaml::from_str(&yaml).expect("deserialize");
        assert_eq!(profile, &parsed, "roundtrip mismatch; yaml was:\n{yaml}");
    }

    #[test]
    fn test_column_profile_serde_roundtrip_all_variants() {
        let mut distribution = BTreeMap::new();
        distribution.insert("paid".to_string(), 61.8);
        distribution.insert("shipped".to_string(), 18.3);

        let mut hourly = BTreeMap::new();
        hourly.insert(0u8, 1.2);
        hourly.insert(13u8, 6.8);

        let mut monthly = BTreeMap::new();
        monthly.insert("2021-04".to_string(), 312u64);
        monthly.insert("2021-05".to_string(), 489u64);

        let variants = vec![
            ColumnProfile::Serial,
            ColumnProfile::SkippedSensitive {
                reason: "password".into(),
            },
            ColumnProfile::SkippedExcluded,
            ColumnProfile::Categorical {
                distribution,
                null_rate: 0.0,
            },
            ColumnProfile::StringStats {
                semantic: Some("email".into()),
                cardinality: 48392,
                null_rate: 0.0,
                avg_length: 22.0,
                min_length: Some(8),
                max_length: Some(64),
            },
            // StringStats with every optional field omitted.
            ColumnProfile::StringStats {
                semantic: None,
                cardinality: 100,
                null_rate: 5.0,
                avg_length: 12.0,
                min_length: None,
                max_length: None,
            },
            ColumnProfile::Numeric {
                min: 0.99,
                max: 9847.50,
                mean: 67.32,
                median: 42.15,
                stddev: 89.44,
                null_rate: 0.0,
                percentiles: Some(sample_percentiles()),
            },
            // Numeric with optional percentiles omitted.
            ColumnProfile::Numeric {
                min: 1.0,
                max: 10.0,
                mean: 5.0,
                median: 5.0,
                stddev: 2.0,
                null_rate: 1.5,
                percentiles: None,
            },
            ColumnProfile::Boolean {
                true_rate: 78.5,
                null_rate: 0.0,
            },
            ColumnProfile::Timestamp {
                range: ("2021-03-15T00:00:00".into(), "2026-06-28T23:59:59".into()),
                null_rate: 0.0,
                weekday_ratio: Some(0.68),
                hourly_density: hourly,
                monthly_density: monthly,
            },
            // Timestamp with optional temporal patterns omitted.
            ColumnProfile::Timestamp {
                range: ("2024-01-01T00:00:00".into(), "2024-12-31T23:59:59".into()),
                null_rate: 2.0,
                weekday_ratio: None,
                hourly_density: BTreeMap::new(),
                monthly_density: BTreeMap::new(),
            },
        ];

        for v in &variants {
            yaml_roundtrip(v);
        }
    }

    #[test]
    fn test_column_profile_type_tag_rendering() {
        let yaml = serde_yaml::to_string(&ColumnProfile::Serial).unwrap();
        assert!(yaml.contains("type: serial"), "got: {yaml}");

        let cat = ColumnProfile::Categorical {
            distribution: BTreeMap::new(),
            null_rate: 0.0,
        };
        let yaml = serde_yaml::to_string(&cat).unwrap();
        assert!(yaml.contains("type: categorical"), "got: {yaml}");
    }

    #[test]
    fn test_parent_ratio_roundtrip() {
        let pr = ParentRatio {
            column: "user_id".into(),
            avg: 7.3,
            min: 0,
            max: 89,
            median: 5.0,
            stddev: 8.2,
            percentiles: Some(sample_percentiles()),
            zero_count: Some(4201),
            zero_rate: Some(8.7),
        };
        let yaml = serde_yaml::to_string(&pr).unwrap();
        let parsed: ParentRatio = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(pr, parsed);
    }

    #[test]
    fn test_database_profile_roundtrip() {
        let mut columns = BTreeMap::new();
        columns.insert("id".to_string(), ColumnProfile::Serial);
        columns.insert(
            "role".to_string(),
            ColumnProfile::Categorical {
                distribution: BTreeMap::from([
                    ("user".to_string(), 89.2),
                    ("admin".to_string(), 10.8),
                ]),
                null_rate: 0.0,
            },
        );

        let mut parent_ratios = BTreeMap::new();
        parent_ratios.insert(
            "users".to_string(),
            ParentRatio {
                column: "user_id".into(),
                avg: 7.3,
                min: 0,
                max: 89,
                median: 5.0,
                stddev: 8.2,
                percentiles: None,
                zero_count: None,
                zero_rate: None,
            },
        );

        let mut tables = BTreeMap::new();
        tables.insert(
            "orders".to_string(),
            TableProfile {
                row_count: 354281,
                parent_ratios,
                columns,
            },
        );

        let profile = DatabaseProfile {
            version: "1.0".into(),
            profiled_at: "2026-06-29T10:00:00Z".into(),
            source_hash: "sha256:abc".into(),
            seedgen_version: "0.3.0".into(),
            options: ProfileOptionsSummary {
                cardinality_threshold: 50,
                skipped_sensitive: vec!["users.password_hash".into()],
            },
            tables,
        };

        let yaml = serde_yaml::to_string(&profile).unwrap();
        let parsed: DatabaseProfile = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(profile, parsed);
    }
}
