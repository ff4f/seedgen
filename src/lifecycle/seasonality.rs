//! Seasonality model — a per-bucket multiplier applied after the growth count.
//!
//! A table may carry exactly one seasonality kind. The multiplier is selected by
//! the bucket's start date. See `LIFECYCLE.md` → `seasonality.rs`.

use chrono::Datelike;

use crate::lifecycle::config::TimeBucket;

/// Which calendar field drives the multiplier lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeasonalityKind {
    /// 12 multipliers, indexed by month (Jan..Dec).
    Monthly,
    /// 4 multipliers, indexed by quarter (Q1..Q4).
    Quarterly,
    /// 7 multipliers, indexed by day of week (Mon..Sun).
    Weekly,
}

/// A seasonal multiplier table.
#[derive(Debug, Clone, PartialEq)]
pub struct SeasonalityModel {
    pub multipliers: Vec<f64>,
    pub kind: SeasonalityKind,
}

impl SeasonalityModel {
    /// Multiplier for `bucket`, selected from its start date. Out-of-range
    /// indices fall back to a neutral `1.0` so a malformed table never panics.
    pub fn multiplier_for(&self, bucket: &TimeBucket) -> f64 {
        let index = match self.kind {
            SeasonalityKind::Monthly => bucket.start.month0() as usize, // 0..11
            SeasonalityKind::Quarterly => (bucket.start.month0() / 3) as usize, // 0..3
            SeasonalityKind::Weekly => {
                bucket.start.weekday().num_days_from_monday() as usize // 0..6
            }
        };
        self.multipliers.get(index).copied().unwrap_or(1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn bucket_starting(y: i32, m: u32, d: u32) -> TimeBucket {
        let start = NaiveDate::from_ymd_opt(y, m, d).expect("valid date");
        TimeBucket {
            index: 0,
            start,
            end: start,
        }
    }

    fn monthly() -> SeasonalityModel {
        SeasonalityModel {
            multipliers: vec![1.0, 0.7, 0.85, 1.0, 1.1, 0.8, 0.7, 0.85, 1.2, 1.4, 1.8, 2.5],
            kind: SeasonalityKind::Monthly,
        }
    }

    #[test]
    fn test_seasonality_december_multiplier() {
        let m = monthly();
        assert_eq!(m.multiplier_for(&bucket_starting(2024, 12, 1)), 2.5);
    }

    #[test]
    fn test_seasonality_july_multiplier() {
        let m = monthly();
        assert_eq!(m.multiplier_for(&bucket_starting(2024, 7, 1)), 0.7);
    }

    #[test]
    fn test_seasonality_january_multiplier() {
        let m = monthly();
        assert_eq!(m.multiplier_for(&bucket_starting(2024, 1, 15)), 1.0);
    }

    #[test]
    fn test_seasonality_quarterly() {
        let m = SeasonalityModel {
            multipliers: vec![1.0, 1.5, 0.8, 2.0],
            kind: SeasonalityKind::Quarterly,
        };
        assert_eq!(m.multiplier_for(&bucket_starting(2024, 2, 1)), 1.0); // Q1
        assert_eq!(m.multiplier_for(&bucket_starting(2024, 5, 1)), 1.5); // Q2
        assert_eq!(m.multiplier_for(&bucket_starting(2024, 8, 1)), 0.8); // Q3
        assert_eq!(m.multiplier_for(&bucket_starting(2024, 11, 1)), 2.0); // Q4
    }

    #[test]
    fn test_seasonality_weekly() {
        let m = SeasonalityModel {
            multipliers: vec![1.0, 1.0, 1.0, 1.0, 1.2, 1.5, 0.5],
            kind: SeasonalityKind::Weekly,
        };
        // 2024-01-01 is a Monday → index 0.
        assert_eq!(m.multiplier_for(&bucket_starting(2024, 1, 1)), 1.0);
        // 2024-01-06 is a Saturday → index 5.
        assert_eq!(m.multiplier_for(&bucket_starting(2024, 1, 6)), 1.5);
        // 2024-01-07 is a Sunday → index 6.
        assert_eq!(m.multiplier_for(&bucket_starting(2024, 1, 7)), 0.5);
    }

    #[test]
    fn test_seasonality_out_of_range_is_neutral() {
        let m = SeasonalityModel {
            multipliers: vec![1.0, 2.0], // too short for monthly
            kind: SeasonalityKind::Monthly,
        };
        assert_eq!(m.multiplier_for(&bucket_starting(2024, 12, 1)), 1.0);
    }
}
