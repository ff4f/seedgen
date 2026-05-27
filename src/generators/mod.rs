pub mod geo;
pub mod identifier;
pub mod network;
pub mod numeric;
pub mod structured;
pub mod temporal;
pub mod text;

use chrono::{NaiveDate, NaiveDateTime};
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};

use crate::semantic::GeneratorType;

pub trait Generator: Send + Sync {
    fn generate(&self, rng: &mut ChaCha8Rng) -> Value;
    fn name(&self) -> &str;
}

pub struct NullGenerator;
impl Generator for NullGenerator {
    fn generate(&self, _rng: &mut ChaCha8Rng) -> Value {
        Value::Null
    }
    fn name(&self) -> &str {
        "null"
    }
}

pub fn create_generator(gen_type: &GeneratorType) -> Box<dyn Generator> {
    match gen_type {
        GeneratorType::Email => Box::new(text::EmailGenerator),
        GeneratorType::FirstName => Box::new(text::FirstNameGenerator),
        GeneratorType::LastName => Box::new(text::LastNameGenerator),
        GeneratorType::FullName => Box::new(text::FullNameGenerator),
        GeneratorType::Username => Box::new(text::UsernameGenerator),
        GeneratorType::Phone => Box::new(text::PhoneGenerator),
        GeneratorType::Url => Box::new(text::UrlGenerator),
        GeneratorType::AvatarUrl => Box::new(text::AvatarUrlGenerator),
        GeneratorType::Password => Box::new(text::PasswordGenerator),
        GeneratorType::Slug => Box::new(text::SlugGenerator),
        GeneratorType::Paragraph => Box::new(text::ParagraphGenerator),
        GeneratorType::Sentence => Box::new(text::SentenceGenerator),
        GeneratorType::CompanyName => Box::new(text::CompanyNameGenerator),

        GeneratorType::Uuid => Box::new(identifier::UuidGenerator),
        GeneratorType::Token => Box::new(identifier::TokenGenerator),
        GeneratorType::Sku => Box::new(identifier::SkuGenerator),

        GeneratorType::Money { min, max } => Box::new(numeric::MoneyGenerator {
            min: *min,
            max: *max,
        }),
        GeneratorType::RandomInt { min, max } => Box::new(numeric::RandomIntGenerator {
            min: *min,
            max: *max,
        }),
        GeneratorType::RandomFloat { min, max } => Box::new(numeric::RandomFloatGenerator {
            min: *min,
            max: *max,
        }),

        GeneratorType::Latitude => Box::new(geo::LatitudeGenerator),
        GeneratorType::Longitude => Box::new(geo::LongitudeGenerator),
        GeneratorType::City => Box::new(geo::CityGenerator),
        GeneratorType::Country => Box::new(geo::CountryGenerator),
        GeneratorType::PostalCode => Box::new(geo::PostalCodeGenerator),
        GeneratorType::StreetAddress => Box::new(geo::StreetAddressGenerator),

        GeneratorType::DatetimePast => Box::new(temporal::DatetimePastGenerator),
        GeneratorType::DatetimeRecent => Box::new(temporal::DatetimeRecentGenerator),
        GeneratorType::DateFuture => Box::new(temporal::DateFutureGenerator),

        GeneratorType::HexColor => Box::new(structured::HexColorGenerator),
        GeneratorType::IPv4 => Box::new(network::IPv4Generator),
        GeneratorType::CurrencyCode => Box::new(structured::CurrencyCodeGenerator),
        GeneratorType::CountryCode => Box::new(geo::CountryCodeGenerator),
        GeneratorType::EnumPick { values } => Box::new(structured::EnumPickGenerator {
            values: values.clone(),
        }),
        GeneratorType::JsonEmpty => Box::new(structured::JsonEmptyGenerator),
        GeneratorType::BoolRandom => Box::new(structured::BoolGenerator),

        GeneratorType::RandomString { max_length } => Box::new(structured::RandomStringGenerator {
            max_length: *max_length,
        }),
        // Both Null and Skip return Value::Null at the generator layer.
        // The output layer is responsible for omitting Skip columns from INSERT entirely.
        GeneratorType::Null | GeneratorType::Skip => Box::new(NullGenerator),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    fn rng(seed: u64) -> ChaCha8Rng {
        ChaCha8Rng::seed_from_u64(seed)
    }

    fn all_variants() -> Vec<GeneratorType> {
        vec![
            GeneratorType::Email,
            GeneratorType::FirstName,
            GeneratorType::LastName,
            GeneratorType::FullName,
            GeneratorType::Username,
            GeneratorType::Phone,
            GeneratorType::Url,
            GeneratorType::AvatarUrl,
            GeneratorType::Password,
            GeneratorType::Slug,
            GeneratorType::Paragraph,
            GeneratorType::Sentence,
            GeneratorType::CompanyName,
            GeneratorType::Uuid,
            GeneratorType::Token,
            GeneratorType::Sku,
            GeneratorType::Money {
                min: 0.0,
                max: 1000.0,
            },
            GeneratorType::RandomInt { min: 0, max: 100 },
            GeneratorType::RandomFloat { min: 0.0, max: 1.0 },
            GeneratorType::Latitude,
            GeneratorType::Longitude,
            GeneratorType::City,
            GeneratorType::Country,
            GeneratorType::PostalCode,
            GeneratorType::StreetAddress,
            GeneratorType::DatetimePast,
            GeneratorType::DatetimeRecent,
            GeneratorType::DateFuture,
            GeneratorType::HexColor,
            GeneratorType::IPv4,
            GeneratorType::CurrencyCode,
            GeneratorType::CountryCode,
            GeneratorType::EnumPick {
                values: vec!["pending".into(), "shipped".into()],
            },
            GeneratorType::JsonEmpty,
            GeneratorType::BoolRandom,
            GeneratorType::RandomString { max_length: 12 },
            GeneratorType::Null,
            GeneratorType::Skip,
        ]
    }

    #[test]
    fn test_create_generator_handles_every_variant() {
        let mut r = rng(42);
        for variant in all_variants() {
            let g = create_generator(&variant);
            let value = g.generate(&mut r);
            // Generators must always return *some* Value — no panics, no garbage.
            // Null/Skip explicitly produce Value::Null; everything else produces a concrete value.
            match (&variant, &value) {
                (GeneratorType::Null, Value::Null) => {}
                (GeneratorType::Skip, Value::Null) => {}
                (_, Value::Null) => panic!("variant {variant:?} unexpectedly returned Null"),
                _ => {}
            }
        }
    }

    #[test]
    fn test_create_generator_is_deterministic_per_variant() {
        for variant in all_variants() {
            let g = create_generator(&variant);
            let mut a = rng(7);
            let mut b = rng(7);
            for _ in 0..5 {
                assert_eq!(
                    g.generate(&mut a),
                    g.generate(&mut b),
                    "{:?} not deterministic",
                    variant
                );
            }
        }
    }

    #[test]
    fn test_create_generator_money_respects_bounds() {
        let g = create_generator(&GeneratorType::Money {
            min: 10.0,
            max: 20.0,
        });
        let mut r = rng(1);
        for _ in 0..30 {
            match g.generate(&mut r) {
                Value::Float(v) => assert!((10.0..=20.0).contains(&v)),
                other => panic!("got {other:?}"),
            }
        }
    }

    #[test]
    fn test_create_generator_random_int_respects_bounds() {
        let g = create_generator(&GeneratorType::RandomInt { min: -5, max: 5 });
        let mut r = rng(1);
        for _ in 0..30 {
            match g.generate(&mut r) {
                Value::Int(i) => assert!((-5..=5).contains(&i)),
                other => panic!("got {other:?}"),
            }
        }
    }

    #[test]
    fn test_create_generator_enum_pick_returns_listed_value() {
        let g = create_generator(&GeneratorType::EnumPick {
            values: vec!["a".into(), "b".into(), "c".into()],
        });
        let mut r = rng(1);
        for _ in 0..30 {
            match g.generate(&mut r) {
                Value::String(s) => assert!(matches!(s.as_str(), "a" | "b" | "c"), "got {s:?}"),
                other => panic!("got {other:?}"),
            }
        }
    }

    #[test]
    fn test_create_generator_random_string_respects_max_length() {
        let g = create_generator(&GeneratorType::RandomString { max_length: 8 });
        let mut r = rng(1);
        for _ in 0..30 {
            match g.generate(&mut r) {
                Value::String(s) => assert!(s.len() <= 8, "got {s:?}"),
                other => panic!("got {other:?}"),
            }
        }
    }

    #[test]
    fn test_create_generator_null_and_skip_return_null() {
        let mut r = rng(1);
        assert_eq!(
            create_generator(&GeneratorType::Null).generate(&mut r),
            Value::Null
        );
        assert_eq!(
            create_generator(&GeneratorType::Skip).generate(&mut r),
            Value::Null
        );
    }

    #[test]
    fn test_create_generator_value_kinds_match_expectations() {
        let mut r = rng(1);
        let cases = [
            (GeneratorType::BoolRandom, "bool"),
            (GeneratorType::Uuid, "uuid"),
            (GeneratorType::JsonEmpty, "json"),
            (GeneratorType::DatetimePast, "timestamp"),
            (GeneratorType::DateFuture, "date"),
            (GeneratorType::RandomInt { min: 0, max: 9 }, "int"),
            (GeneratorType::RandomFloat { min: 0.0, max: 1.0 }, "float"),
        ];
        for (variant, expected_kind) in cases {
            let v = create_generator(&variant).generate(&mut r);
            let actual_kind = match v {
                Value::Bool(_) => "bool",
                Value::Uuid(_) => "uuid",
                Value::Json(_) => "json",
                Value::Timestamp(_) => "timestamp",
                Value::Date(_) => "date",
                Value::Int(_) => "int",
                Value::Float(_) => "float",
                Value::String(_) => "string",
                Value::Null => "null",
            };
            assert_eq!(actual_kind, expected_kind, "variant {variant:?}");
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Value {
    String(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    Null,
    Uuid(String),
    Timestamp(NaiveDateTime),
    Date(NaiveDate),
    Json(serde_json::Value),
}

impl Value {
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    pub fn as_key(&self) -> String {
        match self {
            Value::Null => "__null__".to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Int(i) => i.to_string(),
            Value::Float(f) => f.to_string(),
            Value::String(s) => s.clone(),
            Value::Uuid(s) => s.clone(),
            Value::Timestamp(dt) => dt.to_string(),
            Value::Date(d) => d.to_string(),
            Value::Json(v) => v.to_string(),
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Int(i) => Some(*i as f64),
            Value::Float(f) => Some(*f),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            Value::Uuid(s) => Some(s),
            _ => None,
        }
    }
}
