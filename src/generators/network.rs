use rand::Rng;
use rand_chacha::ChaCha8Rng;

use super::{Generator, Value};

pub use super::text::UrlGenerator;

pub struct IPv4Generator;
impl Generator for IPv4Generator {
    fn generate(&self, rng: &mut ChaCha8Rng) -> Value {
        let a: u8 = rng.gen_range(1..=254);
        let b: u8 = rng.gen_range(0..=255);
        let c: u8 = rng.gen_range(0..=255);
        let d: u8 = rng.gen_range(1..=254);
        Value::String(format!("{a}.{b}.{c}.{d}"))
    }
    fn name(&self) -> &str {
        "ipv4"
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
    fn test_ipv4_is_deterministic() {
        let g = IPv4Generator;
        let mut a = rng(5);
        let mut b = rng(5);
        for _ in 0..20 {
            assert_eq!(g.generate(&mut a), g.generate(&mut b));
        }
    }

    #[test]
    fn test_ipv4_format_and_bounds() {
        let g = IPv4Generator;
        let mut r = rng(1);
        for _ in 0..50 {
            let s = match g.generate(&mut r) {
                Value::String(s) => s,
                other => panic!("expected String, got {other:?}"),
            };
            let parts: Vec<&str> = s.split('.').collect();
            assert_eq!(parts.len(), 4, "got {s:?}");
            let nums: Vec<u32> = parts.iter().map(|p| p.parse::<u32>().unwrap()).collect();
            assert!(nums[0] >= 1 && nums[0] <= 254, "first octet: {s}");
            assert!(nums[1] <= 255, "got {s}");
            assert!(nums[2] <= 255, "got {s}");
            assert!(nums[3] >= 1 && nums[3] <= 254, "last octet: {s}");
        }
    }

    #[test]
    fn test_url_reexport_still_works() {
        let g = UrlGenerator;
        let mut r = rng(1);
        match g.generate(&mut r) {
            Value::String(s) => assert!(s.starts_with("https://"), "got {s:?}"),
            other => panic!("expected String, got {other:?}"),
        }
    }
}
