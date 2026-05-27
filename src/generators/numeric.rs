use rand::Rng;
use rand_chacha::ChaCha8Rng;

use super::{Generator, Value};

pub struct MoneyGenerator {
    pub min: f64,
    pub max: f64,
}
impl Generator for MoneyGenerator {
    fn generate(&self, rng: &mut ChaCha8Rng) -> Value {
        let raw: f64 = rng.gen_range(self.min..=self.max);
        Value::Float((raw * 100.0).round() / 100.0)
    }
    fn name(&self) -> &str {
        "money"
    }
}

pub struct RandomIntGenerator {
    pub min: i64,
    pub max: i64,
}
impl Generator for RandomIntGenerator {
    fn generate(&self, rng: &mut ChaCha8Rng) -> Value {
        Value::Int(rng.gen_range(self.min..=self.max))
    }
    fn name(&self) -> &str {
        "random_int"
    }
}

pub struct RandomFloatGenerator {
    pub min: f64,
    pub max: f64,
}
impl Generator for RandomFloatGenerator {
    fn generate(&self, rng: &mut ChaCha8Rng) -> Value {
        Value::Float(rng.gen_range(self.min..=self.max))
    }
    fn name(&self) -> &str {
        "random_float"
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
    fn test_money_is_deterministic() {
        let g = MoneyGenerator {
            min: 0.0,
            max: 1000.0,
        };
        let mut a = rng(42);
        let mut b = rng(42);
        for _ in 0..20 {
            assert_eq!(g.generate(&mut a), g.generate(&mut b));
        }
    }

    #[test]
    fn test_money_in_range_with_two_decimals() {
        let g = MoneyGenerator {
            min: 5.0,
            max: 100.0,
        };
        let mut r = rng(7);
        for _ in 0..50 {
            match g.generate(&mut r) {
                Value::Float(v) => {
                    assert!((5.0..=100.0).contains(&v), "{v} out of range");
                    let cents = (v * 100.0).round();
                    assert!((cents - v * 100.0).abs() < 1e-9, "not 2-dp: {v}");
                }
                other => panic!("expected Float, got {other:?}"),
            }
        }
    }

    #[test]
    fn test_random_int_is_deterministic() {
        let g = RandomIntGenerator { min: -50, max: 50 };
        let mut a = rng(99);
        let mut b = rng(99);
        for _ in 0..20 {
            assert_eq!(g.generate(&mut a), g.generate(&mut b));
        }
    }

    #[test]
    fn test_random_int_respects_bounds() {
        let g = RandomIntGenerator { min: -10, max: 10 };
        let mut r = rng(1);
        for _ in 0..100 {
            match g.generate(&mut r) {
                Value::Int(i) => assert!((-10..=10).contains(&i)),
                other => panic!("expected Int, got {other:?}"),
            }
        }
    }

    #[test]
    fn test_random_float_is_deterministic() {
        let g = RandomFloatGenerator { min: 0.0, max: 1.0 };
        let mut a = rng(3);
        let mut b = rng(3);
        for _ in 0..20 {
            assert_eq!(g.generate(&mut a), g.generate(&mut b));
        }
    }

    #[test]
    fn test_random_float_respects_bounds() {
        let g = RandomFloatGenerator {
            min: -1.0,
            max: 1.0,
        };
        let mut r = rng(1);
        for _ in 0..100 {
            match g.generate(&mut r) {
                Value::Float(f) => assert!((-1.0..=1.0).contains(&f)),
                other => panic!("expected Float, got {other:?}"),
            }
        }
    }
}
