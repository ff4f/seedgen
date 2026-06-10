//! Churn model — which existing entities go inactive in a given bucket.
//!
//! Eligibility requires the entity to be active and to have survived at least
//! `grace_period` buckets. Among eligible entities, each churns with probability
//! `rate`, drawn from the seeded `ChaCha8Rng`. See `LIFECYCLE.md` → `churn.rs`.

use chrono::NaiveDateTime;
use rand::Rng;
use rand_chacha::ChaCha8Rng;

use crate::generators::Value;
use crate::lifecycle::config::{TimeBucket, TrackedEntity};

/// How entities leave the active population over time.
#[derive(Debug, Clone, PartialEq)]
pub struct ChurnModel {
    /// Probability in `[0, 1]` that an eligible entity churns this bucket.
    pub rate: f64,
    /// Minimum buckets an entity must survive before it is eligible to churn.
    pub grace_period: usize,
    /// Column toggled when an entity churns (e.g. `is_active`, `deleted_at`).
    pub column: String,
    /// Value written to `column` on churn (e.g. `false`, `"churned"`).
    pub value: Value,
    /// When true, child tables stop generating for churned entities. The engine
    /// consumes this flag; the churn model only records it.
    pub cascade: bool,
}

/// A single churn transition for one entity in one bucket.
#[derive(Debug, Clone, PartialEq)]
pub struct ChurnEvent {
    pub entity_id: i64,
    pub bucket_index: usize,
    pub churned_at: NaiveDateTime,
}

impl ChurnModel {
    /// Given the currently active entities and the current bucket, return the
    /// entities that churn this bucket as [`ChurnEvent`]s.
    ///
    /// Entities are eligible only if they are active and at least `grace_period`
    /// buckets old. The rng is consumed only for eligible entities when `rate`
    /// is strictly between 0 and 1, keeping the stream predictable at the 0% and
    /// 100% extremes.
    pub fn apply(
        &self,
        active_entities: &[TrackedEntity],
        bucket: &TimeBucket,
        rng: &mut ChaCha8Rng,
    ) -> Vec<ChurnEvent> {
        let mut events = Vec::new();
        for entity in active_entities {
            if !entity.is_active {
                continue;
            }
            if entity.age_in_buckets(bucket.index) < self.grace_period {
                continue;
            }
            if self.churns(rng) {
                events.push(ChurnEvent {
                    entity_id: entity.id,
                    bucket_index: bucket.index,
                    churned_at: bucket.random_datetime(rng),
                });
            }
        }
        events
    }

    /// Decide whether one eligible entity churns. Avoids touching the rng at the
    /// deterministic extremes so 0% and 100% rates are exact.
    fn churns(&self, rng: &mut ChaCha8Rng) -> bool {
        if self.rate <= 0.0 {
            false
        } else if self.rate >= 1.0 {
            true
        } else {
            rng.gen_bool(self.rate)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use rand::SeedableRng;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).expect("valid date")
    }

    fn bucket(index: usize) -> TimeBucket {
        TimeBucket {
            index,
            start: date(2024, 1, 1),
            end: date(2024, 2, 1),
        }
    }

    fn entity(id: i64, created_bucket: usize) -> TrackedEntity {
        TrackedEntity {
            id,
            created_at: date(2024, 1, 1).and_time(chrono::NaiveTime::MIN),
            created_bucket,
            is_active: true,
            churned_at: None,
        }
    }

    fn model(rate: f64, grace_period: usize) -> ChurnModel {
        ChurnModel {
            rate,
            grace_period,
            column: "is_active".into(),
            value: Value::Bool(false),
            cascade: true,
        }
    }

    #[test]
    fn test_churn_respects_grace_period() {
        let churn = model(1.0, 3); // 100% rate, 3-bucket grace
        let entities = vec![entity(1, 0), entity(2, 2)];
        let mut rng = ChaCha8Rng::seed_from_u64(42);

        // At bucket 2: entity1 age 2, entity2 age 0 — neither past grace of 3.
        let events = churn.apply(&entities, &bucket(2), &mut rng);
        assert!(events.is_empty());

        // At bucket 5: entity1 age 5, entity2 age 3 — both eligible, both churn.
        let events = churn.apply(&entities, &bucket(5), &mut rng);
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn test_churn_full_rate_churns_all_eligible() {
        let churn = model(1.0, 0);
        let entities = vec![entity(1, 0), entity(2, 0), entity(3, 0)];
        let mut rng = ChaCha8Rng::seed_from_u64(42);

        let events = churn.apply(&entities, &bucket(4), &mut rng);
        assert_eq!(events.len(), 3);
        let ids: Vec<i64> = events.iter().map(|e| e.entity_id).collect();
        assert_eq!(ids, vec![1, 2, 3]);
    }

    #[test]
    fn test_churn_zero_rate_churns_none() {
        let churn = model(0.0, 0);
        let entities = vec![entity(1, 0), entity(2, 0)];
        let mut rng = ChaCha8Rng::seed_from_u64(42);

        let events = churn.apply(&entities, &bucket(4), &mut rng);
        assert!(events.is_empty());
    }

    #[test]
    fn test_churn_skips_already_inactive() {
        let churn = model(1.0, 0);
        let mut e = entity(1, 0);
        e.is_active = false;
        let mut rng = ChaCha8Rng::seed_from_u64(42);

        let events = churn.apply(&[e], &bucket(4), &mut rng);
        assert!(events.is_empty());
    }

    #[test]
    fn test_churn_event_timestamp_within_bucket() {
        let churn = model(1.0, 0);
        let b = bucket(4);
        let mut rng = ChaCha8Rng::seed_from_u64(42);

        let events = churn.apply(&[entity(1, 0)], &b, &mut rng);
        assert_eq!(events.len(), 1);
        let ts = events[0].churned_at;
        assert!(ts >= b.start_datetime());
        assert!(ts < b.end_datetime());
    }

    #[test]
    fn test_churn_partial_rate_is_deterministic() {
        let churn = model(0.5, 0);
        let entities: Vec<TrackedEntity> = (0..100).map(|i| entity(i, 0)).collect();

        let mut r1 = ChaCha8Rng::seed_from_u64(7);
        let mut r2 = ChaCha8Rng::seed_from_u64(7);
        let e1 = churn.apply(&entities, &bucket(4), &mut r1);
        let e2 = churn.apply(&entities, &bucket(4), &mut r2);

        let ids1: Vec<i64> = e1.iter().map(|e| e.entity_id).collect();
        let ids2: Vec<i64> = e2.iter().map(|e| e.entity_id).collect();
        assert_eq!(ids1, ids2);
        // ~50% should churn; assert it's neither all nor none.
        assert!(!e1.is_empty() && e1.len() < 100);
    }
}
