//! Growth models — how many NEW entities appear in a given time bucket.
//!
//! All models are deterministic: for the deterministic-shape models (Linear,
//! Exponential, SCurve, Logistic, Custom) the result depends only on the model
//! parameters and the bucket index. The `Follows` model derives its count from
//! the active parent count and may apply seeded variance via `ChaCha8Rng`.
//!
//! See `LIFECYCLE.md` → "Rust Data Structures" → `growth.rs`.

use rand::Rng;
use rand_chacha::ChaCha8Rng;

/// How the entity count for a table grows across time buckets.
#[derive(Debug, Clone, PartialEq)]
pub enum GrowthModel {
    /// `initial + rate * bucket_index` — constant additive growth per bucket.
    Linear { initial: f64, rate: f64 },

    /// `initial * (1 + rate) ^ bucket_index` — compounding growth per bucket.
    Exponential { initial: f64, rate: f64 },

    /// Logistic S-curve with an auto-derived midpoint so that bucket 0 ≈ `initial`
    /// and the curve plateaus at `capacity`.
    SCurve {
        initial: f64,
        capacity: f64,
        rate: f64,
    },

    /// Logistic curve with an explicit `midpoint` (bucket index of the inflection).
    Logistic {
        initial: f64,
        capacity: f64,
        rate: f64,
        midpoint: f64,
    },

    /// Explicit per-bucket counts. Buckets beyond the supplied values yield 0.
    Custom { values: Vec<usize> },

    /// Count is proportional to the ACTIVE parent count for the bucket.
    Follows {
        parent_table: String,
        /// Average children per active parent (used when `per_parent` is `None`).
        ratio: Option<f64>,
        /// Inclusive `(min, max)` children per active parent.
        per_parent: Option<(usize, usize)>,
        /// Fractional variance applied to the `ratio` path (0.35 = ±35%).
        variance: f64,
    },
}

impl GrowthModel {
    /// Whether `count_at` returns a CUMULATIVE target population (the curve
    /// models) rather than a per-bucket new count. Cumulative models require the
    /// caller to take the delta between consecutive buckets to get new entities;
    /// `Custom` and `Follows` already yield per-bucket counts directly.
    pub fn is_cumulative(&self) -> bool {
        matches!(
            self,
            GrowthModel::Linear { .. }
                | GrowthModel::Exponential { .. }
                | GrowthModel::SCurve { .. }
                | GrowthModel::Logistic { .. }
        )
    }

    /// Calculate the target entity count for `bucket_index`. For cumulative
    /// models (see [`is_cumulative`](Self::is_cumulative)) this is the total
    /// population by that bucket; for `Custom`/`Follows` it is the new count for
    /// that bucket.
    ///
    /// `active_parent_count` is only consulted by the [`GrowthModel::Follows`]
    /// variant; the other variants ignore it. Pass `None` when there is no
    /// tracked parent yet — `Follows` then yields 0.
    pub fn count_at(
        &self,
        bucket_index: usize,
        active_parent_count: Option<usize>,
        rng: &mut ChaCha8Rng,
    ) -> usize {
        match self {
            GrowthModel::Linear { initial, rate } => {
                let count = initial + rate * bucket_index as f64;
                non_negative_round(count)
            }
            GrowthModel::Exponential { initial, rate } => {
                let count = initial * (1.0 + rate).powi(bucket_index as i32);
                non_negative_round(count)
            }
            GrowthModel::SCurve {
                initial,
                capacity,
                rate,
            } => {
                // Derive the midpoint so the curve passes through ~`initial` at
                // bucket 0 and approaches `capacity` asymptotically.
                let midpoint = if *initial > 0.0 && *capacity > 0.0 && *rate != 0.0 {
                    (capacity / initial).ln() / rate
                } else {
                    0.0
                };
                logistic_count(*capacity, *rate, *initial, midpoint, bucket_index)
            }
            GrowthModel::Logistic {
                initial,
                capacity,
                rate,
                midpoint,
            } => logistic_count(*capacity, *rate, *initial, *midpoint, bucket_index),
            GrowthModel::Custom { values } => values.get(bucket_index).copied().unwrap_or(0),
            GrowthModel::Follows {
                ratio,
                per_parent,
                variance,
                ..
            } => follows_count(
                active_parent_count.unwrap_or(0),
                *ratio,
                *per_parent,
                *variance,
                rng,
            ),
        }
    }
}

/// Evaluate a logistic curve `capacity / (1 + exp(-rate * (idx - midpoint)))`,
/// clamped into `[0, capacity]`.
fn logistic_count(
    capacity: f64,
    rate: f64,
    initial: f64,
    midpoint: f64,
    bucket_index: usize,
) -> usize {
    if capacity <= 0.0 {
        return 0;
    }
    let exponent = -rate * (bucket_index as f64 - midpoint);
    let value = capacity / (1.0 + exponent.exp());
    // The asymptote never reaches `capacity`, but rounding could; clamp defensively.
    // Likewise never report fewer than the seed population `initial` at bucket 0.
    let floor = if bucket_index == 0 {
        non_negative_round(initial)
    } else {
        0
    };
    let cap = non_negative_round(capacity);
    non_negative_round(value).clamp(floor.min(cap), cap)
}

/// Proportional count for `Follows`: either per-parent summation (with built-in
/// variance) or `active_parent_count * ratio` with optional symmetric variance.
fn follows_count(
    active_parent_count: usize,
    ratio: Option<f64>,
    per_parent: Option<(usize, usize)>,
    variance: f64,
    rng: &mut ChaCha8Rng,
) -> usize {
    if active_parent_count == 0 {
        return 0;
    }

    if let Some((min, max)) = per_parent {
        let (lo, hi) = if min <= max { (min, max) } else { (max, min) };
        if lo == hi {
            return active_parent_count.saturating_mul(lo);
        }
        let mut total = 0usize;
        for _ in 0..active_parent_count {
            total = total.saturating_add(rng.gen_range(lo..=hi));
        }
        return total;
    }

    let ratio = ratio.unwrap_or(0.0);
    let base = active_parent_count as f64 * ratio;
    let factor = if variance > 0.0 {
        1.0 + rng.gen_range(-variance..=variance)
    } else {
        1.0
    };
    non_negative_round(base * factor)
}

/// Round to the nearest whole number, treating negative / non-finite results as 0.
fn non_negative_round(value: f64) -> usize {
    if !value.is_finite() || value < 0.0 {
        0
    } else {
        value.round() as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    fn rng() -> ChaCha8Rng {
        ChaCha8Rng::seed_from_u64(42)
    }

    #[test]
    fn test_growth_linear_at_buckets_0_5_10() {
        let model = GrowthModel::Linear {
            initial: 10.0,
            rate: 5.0,
        };
        let mut r = rng();
        assert_eq!(model.count_at(0, None, &mut r), 10); // 10
        assert_eq!(model.count_at(5, None, &mut r), 35); // 10 + 25
        assert_eq!(model.count_at(10, None, &mut r), 60); // 10 + 50
    }

    #[test]
    fn test_growth_exponential_compounding_at_0_1_12() {
        let model = GrowthModel::Exponential {
            initial: 100.0,
            rate: 0.10,
        };
        let mut r = rng();
        assert_eq!(model.count_at(0, None, &mut r), 100); // 100
        assert_eq!(model.count_at(1, None, &mut r), 110); // 100 * 1.1
        assert_eq!(model.count_at(12, None, &mut r), 314); // 100 * 1.1^12 = 313.84
    }

    #[test]
    fn test_growth_s_curve_slow_then_accelerates_then_plateaus() {
        let model = GrowthModel::SCurve {
            initial: 10.0,
            capacity: 1000.0,
            rate: 0.3,
        };
        let mut r = rng();
        let start = model.count_at(0, None, &mut r);
        let early = model.count_at(5, None, &mut r);
        let mid = model.count_at(15, None, &mut r);
        let late = model.count_at(50, None, &mut r);

        assert_eq!(start, 10); // begins near `initial`
        assert!(early < 200, "early bucket should still be small: {early}");
        assert!(
            mid > early,
            "should accelerate past the early bucket: {mid}"
        );
        assert!(late > 950, "should approach capacity: {late}");
        assert!(late <= 1000, "must never exceed capacity: {late}");
    }

    #[test]
    fn test_growth_logistic_plateaus_at_capacity() {
        let model = GrowthModel::Logistic {
            initial: 10.0,
            capacity: 500.0,
            rate: 0.25,
            midpoint: 12.0,
        };
        let mut r = rng();
        let late = model.count_at(60, None, &mut r);
        assert!(late > 480, "should approach capacity: {late}");
        assert!(late <= 500, "must never exceed capacity: {late}");
    }

    #[test]
    fn test_growth_custom_returns_explicit_values() {
        let model = GrowthModel::Custom {
            values: vec![3, 7, 11, 0, 42],
        };
        let mut r = rng();
        assert_eq!(model.count_at(0, None, &mut r), 3);
        assert_eq!(model.count_at(2, None, &mut r), 11);
        assert_eq!(model.count_at(4, None, &mut r), 42);
        // Out of range → 0
        assert_eq!(model.count_at(5, None, &mut r), 0);
    }

    #[test]
    fn test_growth_follows_ratio_no_variance_is_proportional() {
        let model = GrowthModel::Follows {
            parent_table: "users".into(),
            ratio: Some(3.0),
            per_parent: None,
            variance: 0.0,
        };
        let mut r = rng();
        assert_eq!(model.count_at(0, Some(100), &mut r), 300);
        assert_eq!(model.count_at(0, Some(0), &mut r), 0);
        assert_eq!(model.count_at(0, None, &mut r), 0);
    }

    #[test]
    fn test_growth_follows_per_parent_within_bounds() {
        let model = GrowthModel::Follows {
            parent_table: "orders".into(),
            ratio: None,
            per_parent: Some((1, 5)),
            variance: 0.0,
        };
        let mut r = rng();
        let count = model.count_at(0, Some(100), &mut r);
        // 100 parents, each contributing 1..=5 children.
        assert!((100..=500).contains(&count), "out of bounds: {count}");
    }

    #[test]
    fn test_growth_follows_variance_stays_in_expected_band() {
        let model = GrowthModel::Follows {
            parent_table: "orders".into(),
            ratio: Some(3.2),
            per_parent: None,
            variance: 0.35,
        };
        let mut r = rng();
        let count = model.count_at(0, Some(1000), &mut r);
        // base = 3200, ±35% → [2080, 4320]
        assert!((2080..=4320).contains(&count), "out of band: {count}");
    }

    #[test]
    fn test_growth_deterministic_same_seed_same_count() {
        let model = GrowthModel::Exponential {
            initial: 50.0,
            rate: 0.15,
        };
        let mut r1 = ChaCha8Rng::seed_from_u64(42);
        let mut r2 = ChaCha8Rng::seed_from_u64(42);
        for i in 0..20 {
            assert_eq!(
                model.count_at(i, None, &mut r1),
                model.count_at(i, None, &mut r2),
            );
        }
    }

    #[test]
    fn test_growth_follows_deterministic_same_seed_same_count() {
        let model = GrowthModel::Follows {
            parent_table: "users".into(),
            ratio: Some(3.2),
            per_parent: None,
            variance: 0.35,
        };
        let mut r1 = ChaCha8Rng::seed_from_u64(7);
        let mut r2 = ChaCha8Rng::seed_from_u64(7);
        for i in 0..20 {
            assert_eq!(
                model.count_at(i, Some(500), &mut r1),
                model.count_at(i, Some(500), &mut r2),
            );
        }
    }
}
