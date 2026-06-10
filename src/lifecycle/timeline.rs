//! Timeline distributions — a categorical distribution that changes over time.
//!
//! Distributions are anchored at dated keyframes; values between keyframes are
//! linearly interpolated per key. See `LIFECYCLE.md` → `timeline.rs` and the
//! "`timeline`" YAML reference.

use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::NaiveDate;

/// A distribution over category labels that evolves across keyframe dates.
#[derive(Debug, Clone, PartialEq)]
pub struct TimelineDistribution {
    /// Dated keyframes, each mapping a category label to a weight.
    pub keyframes: BTreeMap<NaiveDate, HashMap<String, f64>>,
}

impl TimelineDistribution {
    /// Distribution at `date`.
    ///
    /// - On or between keyframes: each key is linearly interpolated between the
    ///   surrounding keyframes (missing keys treated as 0 on either side).
    /// - Before the first keyframe: the first keyframe is returned as-is.
    /// - After the last keyframe: the last keyframe is returned as-is.
    /// - No keyframes: an empty map.
    pub fn distribution_at(&self, date: NaiveDate) -> HashMap<String, f64> {
        let before = self.keyframes.range(..=date).next_back();
        let after = self.keyframes.range(date..).next();

        match (before, after) {
            (Some((d1, dist1)), Some((d2, dist2))) => {
                if d1 == d2 {
                    // `date` lands exactly on a keyframe.
                    return dist1.clone();
                }
                let total = (*d2 - *d1).num_days() as f64;
                let elapsed = (date - *d1).num_days() as f64;
                let t = if total > 0.0 { elapsed / total } else { 0.0 };

                let keys: HashSet<&String> = dist1.keys().chain(dist2.keys()).collect();
                let mut result = HashMap::with_capacity(keys.len());
                for key in keys {
                    let v1 = dist1.get(key).copied().unwrap_or(0.0);
                    let v2 = dist2.get(key).copied().unwrap_or(0.0);
                    result.insert(key.clone(), v1 + (v2 - v1) * t);
                }
                result
            }
            (Some((_, dist)), None) => dist.clone(),
            (None, Some((_, dist))) => dist.clone(),
            (None, None) => HashMap::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).expect("valid date")
    }

    fn two_keyframe_timeline() -> TimelineDistribution {
        let mut keyframes = BTreeMap::new();
        keyframes.insert(
            date(2023, 1, 1),
            HashMap::from([("pro".to_string(), 80.0), ("free".to_string(), 20.0)]),
        );
        keyframes.insert(
            date(2025, 1, 1),
            HashMap::from([
                ("pro".to_string(), 20.0),
                ("free".to_string(), 60.0),
                ("enterprise".to_string(), 20.0),
            ]),
        );
        TimelineDistribution { keyframes }
    }

    fn close(a: f64, b: f64) {
        assert!((a - b).abs() < 1.0, "{a} != {b}");
    }

    #[test]
    fn test_timeline_midpoint_interpolation() {
        let timeline = two_keyframe_timeline();
        // 2024-01-01 is ~the midpoint between 2023-01-01 and 2025-01-01.
        let dist = timeline.distribution_at(date(2024, 1, 1));
        close(dist["pro"], 50.0); // lerp(80, 20, 0.5)
        close(dist["free"], 40.0); // lerp(20, 60, 0.5)
        close(dist["enterprise"], 10.0); // lerp(0, 20, 0.5)
    }

    #[test]
    fn test_timeline_exact_keyframe() {
        let timeline = two_keyframe_timeline();
        let dist = timeline.distribution_at(date(2023, 1, 1));
        close(dist["pro"], 80.0);
        close(dist["free"], 20.0);
        assert!(!dist.contains_key("enterprise"));
    }

    #[test]
    fn test_timeline_before_first_keyframe() {
        let timeline = two_keyframe_timeline();
        let dist = timeline.distribution_at(date(2020, 6, 1));
        close(dist["pro"], 80.0);
        close(dist["free"], 20.0);
    }

    #[test]
    fn test_timeline_after_last_keyframe() {
        let timeline = two_keyframe_timeline();
        let dist = timeline.distribution_at(date(2030, 12, 31));
        close(dist["pro"], 20.0);
        close(dist["free"], 60.0);
        close(dist["enterprise"], 20.0);
    }

    #[test]
    fn test_timeline_quarter_point_interpolation() {
        let timeline = two_keyframe_timeline();
        // 2023-07-02 ≈ 25% of the way from 2023-01-01 to 2025-01-01.
        let dist = timeline.distribution_at(date(2023, 7, 2));
        // pro: lerp(80, 20, 0.25) = 65
        close(dist["pro"], 65.0);
        // enterprise: lerp(0, 20, 0.25) = 5
        close(dist["enterprise"], 5.0);
    }

    #[test]
    fn test_timeline_empty_is_empty() {
        let timeline = TimelineDistribution {
            keyframes: BTreeMap::new(),
        };
        assert!(timeline.distribution_at(date(2024, 1, 1)).is_empty());
    }
}
