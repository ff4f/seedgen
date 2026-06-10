//! Entity pool — tracks the entities generated for one table across buckets so
//! that growth (`follows`), churn, and temporal constraints can reason about the
//! active population. See `LIFECYCLE.md` → `pool.rs` / `engine.rs`.

use std::collections::HashSet;

use chrono::NaiveDateTime;
use rand_chacha::ChaCha8Rng;

use crate::generators::Value;
use crate::lifecycle::churn::{ChurnEvent, ChurnModel};
use crate::lifecycle::config::{TimeBucket, TrackedEntity};
use crate::output::{format_value, quote_ident};

/// All tracked entities for a single table, accumulated bucket by bucket.
#[derive(Debug, Clone)]
pub struct EntityPool {
    pub table: String,
    pub entities: Vec<TrackedEntity>,
}

impl EntityPool {
    pub fn new(table: impl Into<String>) -> Self {
        Self {
            table: table.into(),
            entities: Vec::new(),
        }
    }

    /// Entities that are still active (not churned).
    pub fn active_entities(&self) -> Vec<&TrackedEntity> {
        self.entities.iter().filter(|e| e.is_active).collect()
    }

    /// Number of active entities — the base for `follows` growth.
    pub fn active_count(&self) -> usize {
        self.entities.iter().filter(|e| e.is_active).count()
    }

    /// Total tracked entities, active or churned.
    pub fn total_count(&self) -> usize {
        self.entities.len()
    }

    /// Find a tracked entity by id.
    pub fn entity(&self, id: i64) -> Option<&TrackedEntity> {
        self.entities.iter().find(|e| e.id == id)
    }

    /// Register new entities with explicit, caller-computed creation timestamps
    /// (used by the engine when a row's `created_at` is bucket-windowed or
    /// derived from a parent via a temporal constraint).
    pub fn add_entities_with(&mut self, entries: &[(i64, NaiveDateTime)], bucket_index: usize) {
        for (id, created_at) in entries {
            self.entities.push(TrackedEntity {
                id: *id,
                created_at: *created_at,
                created_bucket: bucket_index,
                is_active: true,
                churned_at: None,
            });
        }
    }

    /// Register newly generated entities for a bucket, assigning each a creation
    /// timestamp within the bucket window. `new_ids` are the id column values
    /// returned by generation (typically `Value::Int`); non-integer ids fall back
    /// to a sequential surrogate so tracking still works.
    pub fn add_entities(&mut self, new_ids: &[Value], bucket: &TimeBucket, rng: &mut ChaCha8Rng) {
        for value in new_ids {
            let id = match value {
                Value::Int(i) => *i,
                _ => self.entities.len() as i64 + 1,
            };
            let created_at = bucket.random_datetime(rng);
            self.entities.push(TrackedEntity {
                id,
                created_at,
                created_bucket: bucket.index,
                is_active: true,
                churned_at: None,
            });
        }
    }

    /// Apply churn events to the pool: flip the matching entities to inactive,
    /// record their churn timestamp, and return the SQL `UPDATE` statement(s)
    /// that persist the change. Returns an empty vec when there are no events.
    pub fn apply_churn(&mut self, events: &[ChurnEvent], churn: &ChurnModel) -> Vec<String> {
        if events.is_empty() {
            return Vec::new();
        }

        let churned: HashSet<i64> = events.iter().map(|e| e.entity_id).collect();
        for entity in &mut self.entities {
            if churned.contains(&entity.id) {
                entity.is_active = false;
                entity.churned_at = events
                    .iter()
                    .find(|e| e.entity_id == entity.id)
                    .map(|e| e.churned_at);
            }
        }

        let id_list = events
            .iter()
            .map(|e| e.entity_id.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let statement = format!(
            "UPDATE {} SET {} = {} WHERE {} IN ({});",
            quote_ident(&self.table),
            quote_ident(&churn.column),
            format_value(&churn.value),
            quote_ident("id"),
            id_list,
        );
        vec![statement]
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

    fn rng() -> ChaCha8Rng {
        ChaCha8Rng::seed_from_u64(42)
    }

    fn ids(range: std::ops::Range<i64>) -> Vec<Value> {
        range.map(Value::Int).collect()
    }

    fn churn_model() -> ChurnModel {
        ChurnModel {
            rate: 1.0,
            grace_period: 0,
            column: "is_active".into(),
            value: Value::Bool(false),
            cascade: true,
        }
    }

    #[test]
    fn test_pool_add_entities_tracks_count_and_bucket() {
        let mut pool = EntityPool::new("users");
        let mut r = rng();
        pool.add_entities(&ids(1..4), &bucket(0), &mut r);
        assert_eq!(pool.total_count(), 3);
        assert_eq!(pool.active_count(), 3);
        assert!(pool.entities.iter().all(|e| e.created_bucket == 0));
        // created_at falls within the bucket window.
        for e in &pool.entities {
            assert!(e.created_at >= bucket(0).start_datetime());
            assert!(e.created_at < bucket(0).end_datetime());
        }
    }

    #[test]
    fn test_pool_active_entities_excludes_churned() {
        let mut pool = EntityPool::new("users");
        let mut r = rng();
        pool.add_entities(&ids(1..6), &bucket(0), &mut r);

        let events = vec![
            ChurnEvent {
                entity_id: 2,
                bucket_index: 1,
                churned_at: bucket(1).start_datetime(),
            },
            ChurnEvent {
                entity_id: 4,
                bucket_index: 1,
                churned_at: bucket(1).start_datetime(),
            },
        ];
        pool.apply_churn(&events, &churn_model());

        assert_eq!(pool.total_count(), 5);
        assert_eq!(pool.active_count(), 3);
        let active_ids: Vec<i64> = pool.active_entities().iter().map(|e| e.id).collect();
        assert_eq!(active_ids, vec![1, 3, 5]);
    }

    #[test]
    fn test_pool_apply_churn_marks_inactive_and_sets_timestamp() {
        let mut pool = EntityPool::new("users");
        let mut r = rng();
        pool.add_entities(&ids(1..3), &bucket(0), &mut r);

        let churned_at = bucket(2).start_datetime();
        let events = vec![ChurnEvent {
            entity_id: 1,
            bucket_index: 2,
            churned_at,
        }];
        pool.apply_churn(&events, &churn_model());

        let e1 = pool.entities.iter().find(|e| e.id == 1).unwrap();
        assert!(!e1.is_active);
        assert_eq!(e1.churned_at, Some(churned_at));
        let e2 = pool.entities.iter().find(|e| e.id == 2).unwrap();
        assert!(e2.is_active);
        assert_eq!(e2.churned_at, None);
    }

    #[test]
    fn test_pool_apply_churn_returns_update_statement() {
        let mut pool = EntityPool::new("users");
        let mut r = rng();
        pool.add_entities(&ids(1..11), &bucket(0), &mut r);

        let events = vec![
            ChurnEvent {
                entity_id: 3,
                bucket_index: 1,
                churned_at: bucket(1).start_datetime(),
            },
            ChurnEvent {
                entity_id: 7,
                bucket_index: 1,
                churned_at: bucket(1).start_datetime(),
            },
        ];
        let stmts = pool.apply_churn(&events, &churn_model());
        assert_eq!(stmts.len(), 1);
        assert_eq!(
            stmts[0],
            "UPDATE \"users\" SET \"is_active\" = FALSE WHERE \"id\" IN (3, 7);"
        );
    }

    #[test]
    fn test_pool_apply_churn_empty_events_no_statement() {
        let mut pool = EntityPool::new("users");
        let stmts = pool.apply_churn(&[], &churn_model());
        assert!(stmts.is_empty());
    }

    #[test]
    fn test_pool_add_entities_accumulates_across_buckets() {
        let mut pool = EntityPool::new("users");
        let mut r = rng();
        pool.add_entities(&ids(1..4), &bucket(0), &mut r);
        pool.add_entities(&ids(4..6), &bucket(1), &mut r);
        assert_eq!(pool.total_count(), 5);
        assert_eq!(pool.entities[4].created_bucket, 1);
    }
}
