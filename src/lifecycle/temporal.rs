//! Temporal constraints — derive a child row's timestamp from its parent's.
//!
//! A constraint references a parent `table.column` and, for `After`/`Before`,
//! an optional offset range sampled from the seeded `ChaCha8Rng`.
//! See `LIFECYCLE.md` → `temporal.rs`.

use chrono::{Duration, NaiveDateTime};
use rand::Rng;
use rand_chacha::ChaCha8Rng;

/// An inclusive `[min, max]` span used to offset a child timestamp.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurationRange {
    pub min: Duration,
    pub max: Duration,
}

impl DurationRange {
    /// Sample a duration uniformly within `[min, max]` (bounds reordered if
    /// supplied backwards). Resolution is one second.
    pub fn sample(&self, rng: &mut ChaCha8Rng) -> Duration {
        let a = self.min.num_seconds();
        let b = self.max.num_seconds();
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        let secs = if lo == hi { lo } else { rng.gen_range(lo..=hi) };
        Duration::seconds(secs)
    }
}

/// How a child column's timestamp relates to a parent column's timestamp.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemporalConstraint {
    /// Child timestamp falls after the parent's, by `offset` (or +0 if none).
    After {
        table: String,
        column: String,
        offset: Option<DurationRange>,
    },
    /// Child timestamp equals the parent's exactly.
    Equals { table: String, column: String },
    /// Child timestamp falls before the parent's, by `offset` (or -0 if none).
    Before {
        table: String,
        column: String,
        offset: Option<DurationRange>,
    },
}

impl TemporalConstraint {
    /// The parent table this constraint references.
    pub fn parent_table(&self) -> &str {
        match self {
            TemporalConstraint::After { table, .. }
            | TemporalConstraint::Before { table, .. }
            | TemporalConstraint::Equals { table, .. } => table,
        }
    }

    /// The parent column this constraint references.
    pub fn parent_column(&self) -> &str {
        match self {
            TemporalConstraint::After { column, .. }
            | TemporalConstraint::Before { column, .. }
            | TemporalConstraint::Equals { column, .. } => column,
        }
    }

    /// Given the parent's timestamp, compute this row's timestamp.
    pub fn resolve(&self, parent_timestamp: NaiveDateTime, rng: &mut ChaCha8Rng) -> NaiveDateTime {
        match self {
            TemporalConstraint::Equals { .. } => parent_timestamp,
            TemporalConstraint::After { offset, .. } => match offset {
                Some(range) => parent_timestamp + range.sample(rng),
                None => parent_timestamp,
            },
            TemporalConstraint::Before { offset, .. } => match offset {
                Some(range) => parent_timestamp - range.sample(rng),
                None => parent_timestamp,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use rand::SeedableRng;

    fn ts(y: i32, m: u32, d: u32, h: u32, min: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, m, d)
            .expect("valid date")
            .and_hms_opt(h, min, 0)
            .expect("valid time")
    }

    fn rng() -> ChaCha8Rng {
        ChaCha8Rng::seed_from_u64(42)
    }

    fn range(min: Duration, max: Duration) -> DurationRange {
        DurationRange { min, max }
    }

    #[test]
    fn test_temporal_after_returns_later_timestamp() {
        let c = TemporalConstraint::After {
            table: "users".into(),
            column: "created_at".into(),
            offset: Some(range(Duration::days(1), Duration::days(30))),
        };
        let parent = ts(2024, 3, 15, 10, 0);
        let mut r = rng();
        for _ in 0..100 {
            let result = c.resolve(parent, &mut r);
            assert!(result > parent, "{result} should be after {parent}");
            assert!(result <= parent + Duration::days(30));
            assert!(result >= parent + Duration::days(1));
        }
    }

    #[test]
    fn test_temporal_after_offset_within_range() {
        let c = TemporalConstraint::After {
            table: "users".into(),
            column: "created_at".into(),
            offset: Some(range(Duration::hours(1), Duration::hours(12))),
        };
        let parent = ts(2024, 3, 15, 0, 0);
        let mut r = rng();
        let result = c.resolve(parent, &mut r);
        let delta = result - parent;
        assert!(delta >= Duration::hours(1));
        assert!(delta <= Duration::hours(12));
    }

    #[test]
    fn test_temporal_equals_returns_same() {
        let c = TemporalConstraint::Equals {
            table: "orders".into(),
            column: "created_at".into(),
        };
        let parent = ts(2024, 3, 15, 10, 30);
        let mut r = rng();
        assert_eq!(c.resolve(parent, &mut r), parent);
    }

    #[test]
    fn test_temporal_before_returns_earlier_timestamp() {
        let c = TemporalConstraint::Before {
            table: "shipments".into(),
            column: "delivered_at".into(),
            offset: Some(range(Duration::days(1), Duration::days(5))),
        };
        let parent = ts(2024, 3, 15, 10, 0);
        let mut r = rng();
        let result = c.resolve(parent, &mut r);
        assert!(result < parent);
        assert!(result >= parent - Duration::days(5));
    }

    #[test]
    fn test_temporal_after_no_offset_returns_parent() {
        let c = TemporalConstraint::After {
            table: "users".into(),
            column: "created_at".into(),
            offset: None,
        };
        let parent = ts(2024, 3, 15, 10, 0);
        let mut r = rng();
        assert_eq!(c.resolve(parent, &mut r), parent);
    }

    #[test]
    fn test_temporal_resolve_is_deterministic() {
        let c = TemporalConstraint::After {
            table: "users".into(),
            column: "created_at".into(),
            offset: Some(range(Duration::days(1), Duration::days(60))),
        };
        let parent = ts(2024, 3, 15, 10, 0);
        let mut r1 = ChaCha8Rng::seed_from_u64(99);
        let mut r2 = ChaCha8Rng::seed_from_u64(99);
        for _ in 0..50 {
            assert_eq!(c.resolve(parent, &mut r1), c.resolve(parent, &mut r2));
        }
    }
}
