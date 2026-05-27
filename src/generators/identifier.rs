use rand::{Rng, RngCore};
use rand_chacha::ChaCha8Rng;

use super::{Generator, Value};

pub struct UuidGenerator;
impl Generator for UuidGenerator {
    fn generate(&self, rng: &mut ChaCha8Rng) -> Value {
        let mut bytes = [0u8; 16];
        rng.fill_bytes(&mut bytes);
        bytes[6] = (bytes[6] & 0x0F) | 0x40;
        bytes[8] = (bytes[8] & 0x3F) | 0x80;
        let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        let uuid = format!(
            "{}-{}-{}-{}-{}",
            &hex[0..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..32],
        );
        Value::Uuid(uuid)
    }
    fn name(&self) -> &str {
        "uuid"
    }
}

pub struct TokenGenerator;
impl Generator for TokenGenerator {
    fn generate(&self, rng: &mut ChaCha8Rng) -> Value {
        let mut bytes = [0u8; 16];
        rng.fill_bytes(&mut bytes);
        let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        Value::String(hex)
    }
    fn name(&self) -> &str {
        "token"
    }
}

pub struct SkuGenerator;
impl Generator for SkuGenerator {
    fn generate(&self, rng: &mut ChaCha8Rng) -> Value {
        let letters: String = (0..3)
            .map(|_| (b'A' + rng.gen_range(0..26)) as char)
            .collect();
        let digits: u32 = rng.gen_range(0..100_000);
        Value::String(format!("{letters}-{digits:05}"))
    }
    fn name(&self) -> &str {
        "sku"
    }
}

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
    fn test_uuid_is_deterministic() {
        assert_deterministic(&UuidGenerator);
    }

    #[test]
    fn test_uuid_v4_format() {
        let g = UuidGenerator;
        let mut r = rng(1);
        for _ in 0..50 {
            let s = match g.generate(&mut r) {
                Value::Uuid(s) => s,
                other => panic!("got {other:?}"),
            };
            assert_eq!(s.len(), 36, "got {s:?}");
            let parts: Vec<&str> = s.split('-').collect();
            assert_eq!(parts.len(), 5);
            assert_eq!(parts[0].len(), 8);
            assert_eq!(parts[1].len(), 4);
            assert_eq!(parts[2].len(), 4);
            assert_eq!(parts[3].len(), 4);
            assert_eq!(parts[4].len(), 12);
            // version 4 marker in first nibble of third group
            assert_eq!(&parts[2][0..1], "4", "version not 4: {s}");
            // variant 10xx in first nibble of fourth group
            let variant_nibble = u8::from_str_radix(&parts[3][0..1], 16).unwrap();
            assert!((variant_nibble & 0xC) == 0x8, "variant bits wrong: {s}");
            assert!(s.chars().all(|c| c == '-' || c.is_ascii_hexdigit()));
        }
    }

    #[test]
    fn test_token_is_deterministic() {
        assert_deterministic(&TokenGenerator);
    }

    #[test]
    fn test_token_is_32_hex_chars() {
        let g = TokenGenerator;
        let mut r = rng(1);
        for _ in 0..20 {
            let t = match g.generate(&mut r) {
                Value::String(s) => s,
                other => panic!("got {other:?}"),
            };
            assert_eq!(t.len(), 32, "got {t:?}");
            assert!(t.chars().all(|c| c.is_ascii_hexdigit()));
        }
    }

    #[test]
    fn test_sku_is_deterministic() {
        assert_deterministic(&SkuGenerator);
    }

    #[test]
    fn test_sku_format() {
        let g = SkuGenerator;
        let mut r = rng(1);
        for _ in 0..20 {
            let s = match g.generate(&mut r) {
                Value::String(s) => s,
                other => panic!("got {other:?}"),
            };
            let parts: Vec<&str> = s.split('-').collect();
            assert_eq!(parts.len(), 2, "got {s:?}");
            assert_eq!(parts[0].len(), 3);
            assert!(parts[0].chars().all(|c| c.is_ascii_uppercase()));
            assert_eq!(parts[1].len(), 5);
            assert!(parts[1].chars().all(|c| c.is_ascii_digit()));
        }
    }
}
