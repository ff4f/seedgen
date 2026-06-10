//! Shared lifecycle data types: the simulation window, bucket granularity, a
//! single time bucket, and the per-entity tracking record.
//!
//! These are foundational types consumed by `churn`, `seasonality`, the entity
//! pool, and the engine. See `LIFECYCLE.md` → "Rust Data Structures".

use chrono::{NaiveDate, NaiveDateTime};
use rand::Rng;
use rand_chacha::ChaCha8Rng;

/// Top-level lifecycle configuration, parsed from the YAML `lifecycle:` block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleConfig {
    /// Inclusive start of the simulation window.
    pub start: NaiveDate,
    /// Inclusive end of the simulation window.
    pub end: NaiveDate,
    /// Granularity of each time bucket.
    pub bucket: BucketGranularity,
}

/// Granularity of a single time bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BucketGranularity {
    Day,
    Week,
    Month,
    Quarter,
}

/// A single time bucket (e.g. March 2024). `start` is inclusive, `end` exclusive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimeBucket {
    /// 0-based bucket number within the simulation.
    pub index: usize,
    /// Inclusive start date of the bucket.
    pub start: NaiveDate,
    /// Exclusive end date of the bucket.
    pub end: NaiveDate,
}

impl TimeBucket {
    /// Midnight at the start of the bucket.
    pub fn start_datetime(&self) -> NaiveDateTime {
        self.start.and_time(chrono::NaiveTime::MIN)
    }

    /// Midnight at the (exclusive) end of the bucket.
    pub fn end_datetime(&self) -> NaiveDateTime {
        self.end.and_time(chrono::NaiveTime::MIN)
    }

    /// Uniformly pick a timestamp within `[start, end)`. If the bucket has zero
    /// or negative span, the start instant is returned.
    pub fn random_datetime(&self, rng: &mut ChaCha8Rng) -> NaiveDateTime {
        let start = self.start_datetime();
        let span = (self.end_datetime() - start).num_seconds();
        if span <= 0 {
            return start;
        }
        start + chrono::Duration::seconds(rng.gen_range(0..span))
    }
}

/// A generated entity tracked across buckets so child tables can reference it
/// and so churn can transition it from active to churned.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackedEntity {
    pub id: i64,
    pub created_at: NaiveDateTime,
    /// Bucket index in which this entity was created.
    pub created_bucket: usize,
    pub is_active: bool,
    pub churned_at: Option<NaiveDateTime>,
}

impl TrackedEntity {
    /// How many buckets old this entity is at `current_bucket` (never negative).
    pub fn age_in_buckets(&self, current_bucket: usize) -> usize {
        current_bucket.saturating_sub(self.created_bucket)
    }
}
