use rand::{Rng, RngCore};
use rand_chacha::ChaCha8Rng;

use super::{Generator, Value};

pub struct BoolGenerator;
impl Generator for BoolGenerator {
    fn generate(&self, rng: &mut ChaCha8Rng) -> Value {
        Value::Bool(rng.gen_bool(0.5))
    }
    fn name(&self) -> &str {
        "bool"
    }
}

pub struct EnumPickGenerator {
    pub values: Vec<String>,
}
impl Generator for EnumPickGenerator {
    fn generate(&self, rng: &mut ChaCha8Rng) -> Value {
        if self.values.is_empty() {
            return Value::Null;
        }
        let idx = rng.gen_range(0..self.values.len());
        Value::String(self.values[idx].clone())
    }
    fn name(&self) -> &str {
        "enum_pick"
    }
}

pub struct JsonEmptyGenerator;
impl Generator for JsonEmptyGenerator {
    fn generate(&self, _rng: &mut ChaCha8Rng) -> Value {
        Value::Json(serde_json::Value::Object(serde_json::Map::new()))
    }
    fn name(&self) -> &str {
        "json_empty"
    }
}

pub struct HexColorGenerator;
impl Generator for HexColorGenerator {
    fn generate(&self, rng: &mut ChaCha8Rng) -> Value {
        let mut bytes = [0u8; 3];
        rng.fill_bytes(&mut bytes);
        Value::String(format!("#{:02X}{:02X}{:02X}", bytes[0], bytes[1], bytes[2]))
    }
    fn name(&self) -> &str {
        "hex_color"
    }
}

pub struct CurrencyCodeGenerator;
impl Generator for CurrencyCodeGenerator {
    fn generate(&self, rng: &mut ChaCha8Rng) -> Value {
        Value::String(CURRENCY_CODES[rng.gen_range(0..CURRENCY_CODES.len())].to_string())
    }
    fn name(&self) -> &str {
        "currency_code"
    }
}

pub struct RandomStringGenerator {
    pub max_length: u32,
}
impl Generator for RandomStringGenerator {
    fn generate(&self, rng: &mut ChaCha8Rng) -> Value {
        let cap = self.max_length.min(16);
        if cap == 0 {
            return Value::String(String::new());
        }
        let len = cap as usize;
        let s: String = (0..len)
            .map(|_| ALPHANUMERIC[rng.gen_range(0..ALPHANUMERIC.len())] as char)
            .collect();
        Value::String(s)
    }
    fn name(&self) -> &str {
        "random_string"
    }
}

const CURRENCY_CODES: &[&str] = &[
    "USD", "EUR", "GBP", "JPY", "CNY", "AUD", "CAD", "CHF", "HKD", "SGD", "SEK", "KRW", "NOK",
    "NZD", "INR", "MXN", "TWD", "ZAR", "BRL", "DKK", "PLN", "THB", "IDR", "HUF", "CZK", "ILS",
    "CLP", "PHP", "AED", "COP", "SAR", "MYR", "RON", "RUB", "TRY", "VND",
];

const ALPHANUMERIC: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    fn rng(seed: u64) -> ChaCha8Rng {
        ChaCha8Rng::seed_from_u64(seed)
    }

    fn assert_deterministic<G: Generator>(g: &G) {
        let mut a = rng(42);
        let mut b = rng(42);
        for _ in 0..20 {
            assert_eq!(g.generate(&mut a), g.generate(&mut b), "{}", g.name());
        }
    }

    #[test]
    fn test_bool_is_deterministic() {
        assert_deterministic(&BoolGenerator);
    }

    #[test]
    fn test_bool_returns_bools() {
        let g = BoolGenerator;
        let mut r = rng(1);
        let mut trues = 0;
        let mut falses = 0;
        for _ in 0..1000 {
            match g.generate(&mut r) {
                Value::Bool(true) => trues += 1,
                Value::Bool(false) => falses += 1,
                other => panic!("got {other:?}"),
            }
        }
        // 50/50 with N=1000 should land somewhere around 500 each.
        assert!(trues > 400 && trues < 600, "trues={trues}, falses={falses}");
    }

    #[test]
    fn test_enum_pick_is_deterministic() {
        let g = EnumPickGenerator {
            values: vec!["a".into(), "b".into(), "c".into()],
        };
        let mut a = rng(42);
        let mut b = rng(42);
        for _ in 0..20 {
            assert_eq!(g.generate(&mut a), g.generate(&mut b));
        }
    }

    #[test]
    fn test_enum_pick_returns_value_from_list() {
        let g = EnumPickGenerator {
            values: vec!["pending".into(), "shipped".into(), "delivered".into()],
        };
        let mut r = rng(1);
        for _ in 0..30 {
            match g.generate(&mut r) {
                Value::String(s) => assert!(g.values.contains(&s)),
                other => panic!("got {other:?}"),
            }
        }
    }

    #[test]
    fn test_enum_pick_empty_returns_null() {
        let g = EnumPickGenerator { values: vec![] };
        let mut r = rng(1);
        assert_eq!(g.generate(&mut r), Value::Null);
    }

    #[test]
    fn test_json_empty_returns_empty_object() {
        let g = JsonEmptyGenerator;
        let mut r = rng(1);
        match g.generate(&mut r) {
            Value::Json(v) => {
                assert!(v.is_object());
                assert_eq!(v.as_object().unwrap().len(), 0);
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn test_hex_color_is_deterministic() {
        assert_deterministic(&HexColorGenerator);
    }

    #[test]
    fn test_hex_color_format() {
        let g = HexColorGenerator;
        let mut r = rng(1);
        for _ in 0..30 {
            let c = match g.generate(&mut r) {
                Value::String(s) => s,
                other => panic!("got {other:?}"),
            };
            assert_eq!(c.len(), 7, "got {c:?}");
            assert!(c.starts_with('#'));
            assert!(c[1..].chars().all(|ch| ch.is_ascii_hexdigit()));
            assert!(c[1..].chars().all(|ch| !ch.is_ascii_lowercase()));
        }
    }

    #[test]
    fn test_currency_code_is_deterministic() {
        assert_deterministic(&CurrencyCodeGenerator);
    }

    #[test]
    fn test_currency_code_is_from_pool() {
        let g = CurrencyCodeGenerator;
        let mut r = rng(1);
        for _ in 0..30 {
            let c = match g.generate(&mut r) {
                Value::String(s) => s,
                other => panic!("got {other:?}"),
            };
            assert_eq!(c.len(), 3);
            assert!(CURRENCY_CODES.contains(&c.as_str()));
        }
    }

    #[test]
    fn test_currency_code_includes_required() {
        for code in ["USD", "EUR", "GBP", "JPY", "IDR"] {
            assert!(CURRENCY_CODES.contains(&code), "missing {code}");
        }
    }

    #[test]
    fn test_random_string_is_deterministic() {
        let g = RandomStringGenerator { max_length: 10 };
        let mut a = rng(42);
        let mut b = rng(42);
        for _ in 0..20 {
            assert_eq!(g.generate(&mut a), g.generate(&mut b));
        }
    }

    #[test]
    fn test_random_string_respects_max_length() {
        for max in [1u32, 5, 10, 16, 100] {
            let g = RandomStringGenerator { max_length: max };
            let mut r = rng(1);
            for _ in 0..20 {
                let s = match g.generate(&mut r) {
                    Value::String(s) => s,
                    other => panic!("got {other:?}"),
                };
                assert!(s.len() <= max as usize, "len {} > max {max}", s.len());
                assert!(
                    s.chars().all(|c| c.is_ascii_alphanumeric()),
                    "non-alphanumeric: {s}"
                );
            }
        }
    }
}
