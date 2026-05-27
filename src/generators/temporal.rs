use chrono::{Duration, NaiveDate, NaiveDateTime};
use rand::Rng;
use rand_chacha::ChaCha8Rng;

use super::{Generator, Value};

const SECONDS_PER_DAY: i64 = 86_400;

fn reference_now() -> NaiveDateTime {
    NaiveDate::from_ymd_opt(2026, 1, 1)
        .expect("valid date")
        .and_hms_opt(0, 0, 0)
        .expect("valid time")
}

pub struct DatetimePastGenerator;
impl Generator for DatetimePastGenerator {
    fn generate(&self, rng: &mut ChaCha8Rng) -> Value {
        let offset: i64 = rng.gen_range(0..(2 * 365 * SECONDS_PER_DAY));
        Value::Timestamp(reference_now() - Duration::seconds(offset))
    }
    fn name(&self) -> &str {
        "datetime_past"
    }
}

pub struct DatetimeRecentGenerator;
impl Generator for DatetimeRecentGenerator {
    fn generate(&self, rng: &mut ChaCha8Rng) -> Value {
        let offset: i64 = rng.gen_range(0..(7 * SECONDS_PER_DAY));
        Value::Timestamp(reference_now() - Duration::seconds(offset))
    }
    fn name(&self) -> &str {
        "datetime_recent"
    }
}

pub struct DateFutureGenerator;
impl Generator for DateFutureGenerator {
    fn generate(&self, rng: &mut ChaCha8Rng) -> Value {
        let offset_days: i64 = rng.gen_range(0..365);
        Value::Date(reference_now().date() + Duration::days(offset_days))
    }
    fn name(&self) -> &str {
        "date_future"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    fn rng(seed: u64) -> ChaCha8Rng {
        ChaCha8Rng::seed_from_u64(seed)
    }

    #[test]
    fn test_datetime_past_is_deterministic() {
        let g = DatetimePastGenerator;
        let mut a = rng(7);
        let mut b = rng(7);
        for _ in 0..20 {
            assert_eq!(g.generate(&mut a), g.generate(&mut b));
        }
    }

    #[test]
    fn test_datetime_past_within_two_years() {
        let g = DatetimePastGenerator;
        let mut r = rng(1);
        let now = reference_now();
        let two_years_ago = now - Duration::days(2 * 365);
        for _ in 0..50 {
            match g.generate(&mut r) {
                Value::Timestamp(dt) => {
                    assert!(dt >= two_years_ago && dt <= now, "{dt} out of window");
                }
                other => panic!("expected Timestamp, got {other:?}"),
            }
        }
    }

    #[test]
    fn test_datetime_recent_is_deterministic() {
        let g = DatetimeRecentGenerator;
        let mut a = rng(11);
        let mut b = rng(11);
        for _ in 0..20 {
            assert_eq!(g.generate(&mut a), g.generate(&mut b));
        }
    }

    #[test]
    fn test_datetime_recent_within_seven_days() {
        let g = DatetimeRecentGenerator;
        let mut r = rng(1);
        let now = reference_now();
        let week_ago = now - Duration::days(7);
        for _ in 0..50 {
            match g.generate(&mut r) {
                Value::Timestamp(dt) => {
                    assert!(dt >= week_ago && dt <= now, "{dt} out of window");
                }
                other => panic!("expected Timestamp, got {other:?}"),
            }
        }
    }

    #[test]
    fn test_date_future_is_deterministic() {
        let g = DateFutureGenerator;
        let mut a = rng(13);
        let mut b = rng(13);
        for _ in 0..20 {
            assert_eq!(g.generate(&mut a), g.generate(&mut b));
        }
    }

    #[test]
    fn test_date_future_within_next_year() {
        let g = DateFutureGenerator;
        let mut r = rng(1);
        let today = reference_now().date();
        let next_year = today + Duration::days(365);
        for _ in 0..50 {
            match g.generate(&mut r) {
                Value::Date(d) => {
                    assert!(d >= today && d <= next_year, "{d} out of window");
                }
                other => panic!("expected Date, got {other:?}"),
            }
        }
    }
}
