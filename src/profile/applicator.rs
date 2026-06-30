//! [`ProfileApplicator`] — the bridge from a [`DatabaseProfile`] to a
//! [`ScenarioConfig`] the existing generation engine already understands.
//!
//! Categorical distributions become `distribution` overrides; numeric ranges
//! become `range` overrides; FK parent-ratios become `per_parent` count
//! expressions. A scale factor shrinks each root table's row count, and child
//! counts follow their parents automatically — so proportional ratios are
//! preserved at any scale.

use std::collections::HashMap;

use crate::profile::errors::ProfileError;
use crate::profile::stats::{ColumnProfile, DatabaseProfile, TableProfile};
use crate::scenario::parser::{ColumnOverride, CountExpression, ScenarioConfig, TableScenario};

/// Converts a profile into scenario overrides, scaled by `scale` in `(0.0, 1.0]`.
pub struct ProfileApplicator {
    profile: DatabaseProfile,
    scale: f64,
}

impl ProfileApplicator {
    /// Create an applicator, validating the scale factor.
    pub fn new(profile: DatabaseProfile, scale: f64) -> Result<Self, ProfileError> {
        Self::validate_scale(scale)?;
        Ok(Self { profile, scale })
    }

    /// Scale must be in `(0.0, 1.0]`.
    pub fn validate_scale(scale: f64) -> Result<(), ProfileError> {
        if scale <= 0.0 || scale > 1.0 {
            return Err(ProfileError::InvalidScale(scale));
        }
        Ok(())
    }

    /// Convert the profile into a [`ScenarioConfig`]. This is where statistics
    /// become generation parameters.
    pub fn to_scenario(&self) -> Result<ScenarioConfig, ProfileError> {
        let mut tables = HashMap::with_capacity(self.profile.tables.len());

        for (table_name, table_profile) in &self.profile.tables {
            let scaled = ((table_profile.row_count as f64) * self.scale).round() as usize;
            let scaled = scaled.max(1); // never generate zero rows

            let mut overrides = HashMap::new();
            for (col_name, col_profile) in &table_profile.columns {
                if let Some(ov) = column_override(col_profile) {
                    overrides.insert(col_name.clone(), ov);
                }
            }

            let count = resolve_count_expression(table_profile, scaled);
            tables.insert(table_name.clone(), TableScenario { count, overrides });
        }

        Ok(ScenarioConfig {
            tables,
            ..Default::default()
        })
    }
}

/// Map a column profile to the override the engine applies, if any.
fn column_override(profile: &ColumnProfile) -> Option<ColumnOverride> {
    match profile {
        ColumnProfile::Categorical { distribution, .. } => {
            let dist: HashMap<String, f64> =
                distribution.iter().map(|(k, v)| (k.clone(), *v)).collect();
            Some(ColumnOverride::Distribution(dist))
        }
        ColumnProfile::Numeric { min, max, .. } => Some(ColumnOverride::Range {
            min: *min,
            max: *max,
        }),
        // Boolean / Timestamp / StringStats / Serial / Skipped: no override yet —
        // the engine falls back to its inferred generator.
        _ => None,
    }
}

/// Determine a table's count expression:
/// - root tables (no parent ratios) → `Fixed(scaled)`
/// - child tables → `PerParent` using the highest-average parent ratio
///
/// The engine evaluates `PerParent` as `parent_count * midpoint(min, max)`, so
/// the band is centered on the captured average (`floor(avg)..ceil(avg)`). With
/// an integer average this is exact; the child count then follows the (already
/// scaled) parent, preserving the ratio at any scale.
fn resolve_count_expression(table_profile: &TableProfile, scaled: usize) -> CountExpression {
    let primary = table_profile.parent_ratios.iter().max_by(|a, b| {
        a.1.avg
            .partial_cmp(&b.1.avg)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    match primary {
        Some((parent_table, ratio)) => {
            let avg = ratio.avg.max(0.0);
            let min = avg.floor() as usize;
            let max = (avg.ceil() as usize).max(min);
            CountExpression::PerParent {
                parent_table: parent_table.clone(),
                min,
                max,
            }
        }
        None => CountExpression::Fixed(scaled),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use crate::profile::config::ProfileOptionsSummary;
    use crate::profile::stats::{DatabaseProfile, ParentRatio, TableProfile};

    fn empty_profile() -> DatabaseProfile {
        DatabaseProfile {
            version: "1.0".into(),
            profiled_at: "2026-06-29T10:00:00Z".into(),
            source_hash: "sha256:test".into(),
            seedgen_version: "0.3.0".into(),
            options: ProfileOptionsSummary {
                cardinality_threshold: 50,
                skipped_sensitive: vec![],
            },
            tables: BTreeMap::new(),
        }
    }

    fn table(row_count: u64) -> TableProfile {
        TableProfile {
            row_count,
            parent_ratios: BTreeMap::new(),
            columns: BTreeMap::new(),
        }
    }

    #[test]
    fn test_validate_scale_bounds() {
        assert!(ProfileApplicator::validate_scale(0.5).is_ok());
        assert!(ProfileApplicator::validate_scale(1.0).is_ok());
        assert!(matches!(
            ProfileApplicator::validate_scale(0.0),
            Err(ProfileError::InvalidScale(_))
        ));
        assert!(matches!(
            ProfileApplicator::validate_scale(-0.1),
            Err(ProfileError::InvalidScale(_))
        ));
        assert!(matches!(
            ProfileApplicator::validate_scale(1.5),
            Err(ProfileError::InvalidScale(_))
        ));
    }

    #[test]
    fn test_root_table_scaled_fixed_count() {
        let mut profile = empty_profile();
        profile.tables.insert("users".into(), table(1000));

        let scenario = ProfileApplicator::new(profile, 0.1)
            .unwrap()
            .to_scenario()
            .unwrap();
        assert_eq!(scenario.tables["users"].count, CountExpression::Fixed(100));
    }

    #[test]
    fn test_scaled_count_has_minimum_one() {
        let mut profile = empty_profile();
        profile.tables.insert("tiny".into(), table(5));
        let scenario = ProfileApplicator::new(profile, 0.01)
            .unwrap()
            .to_scenario()
            .unwrap();
        // round(0.05) == 0 → clamped to 1.
        assert_eq!(scenario.tables["tiny"].count, CountExpression::Fixed(1));
    }

    #[test]
    fn test_child_table_uses_per_parent_centered_on_avg() {
        let mut profile = empty_profile();
        profile.tables.insert("users".into(), table(1000));
        let mut posts = table(5000);
        posts.parent_ratios.insert(
            "users".into(),
            ParentRatio {
                column: "user_id".into(),
                avg: 5.0,
                min: 5,
                max: 5,
                median: 5.0,
                stddev: 0.0,
                percentiles: None,
                zero_count: None,
                zero_rate: None,
            },
        );
        profile.tables.insert("posts".into(), posts);

        let scenario = ProfileApplicator::new(profile, 0.1)
            .unwrap()
            .to_scenario()
            .unwrap();
        // Root scales; child follows its parent via per_parent (avg 5 → 5..5).
        assert_eq!(scenario.tables["users"].count, CountExpression::Fixed(100));
        assert_eq!(
            scenario.tables["posts"].count,
            CountExpression::PerParent {
                parent_table: "users".into(),
                min: 5,
                max: 5,
            }
        );
    }

    #[test]
    fn test_categorical_maps_to_distribution_override() {
        let mut profile = empty_profile();
        let mut t = table(100);
        t.columns.insert(
            "status".into(),
            ColumnProfile::Categorical {
                distribution: BTreeMap::from([("paid".into(), 60.0), ("pending".into(), 40.0)]),
                null_rate: 0.0,
            },
        );
        profile.tables.insert("orders".into(), t);

        let scenario = ProfileApplicator::new(profile, 1.0)
            .unwrap()
            .to_scenario()
            .unwrap();
        match &scenario.tables["orders"].overrides["status"] {
            ColumnOverride::Distribution(d) => {
                assert_eq!(d["paid"], 60.0);
                assert_eq!(d["pending"], 40.0);
            }
            other => panic!("expected Distribution, got {other:?}"),
        }
    }

    #[test]
    fn test_numeric_maps_to_range_override() {
        let mut profile = empty_profile();
        let mut t = table(100);
        t.columns.insert(
            "amount".into(),
            ColumnProfile::Numeric {
                min: 1.0,
                max: 999.0,
                mean: 50.0,
                median: 42.0,
                stddev: 10.0,
                null_rate: 0.0,
                percentiles: None,
            },
        );
        profile.tables.insert("orders".into(), t);

        let scenario = ProfileApplicator::new(profile, 1.0)
            .unwrap()
            .to_scenario()
            .unwrap();
        assert_eq!(
            scenario.tables["orders"].overrides["amount"],
            ColumnOverride::Range {
                min: 1.0,
                max: 999.0
            }
        );
    }
}
