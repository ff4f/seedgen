use crate::introspection::{Column, DataType, EnumType};

#[derive(Debug, Clone, PartialEq)]
pub enum GeneratorType {
    Email,
    FirstName,
    LastName,
    FullName,
    Username,
    Phone,
    Url,
    AvatarUrl,
    Password,
    Slug,
    Paragraph,
    Sentence,
    CompanyName,

    Uuid,
    Token,
    Sku,

    Money { min: f64, max: f64 },
    RandomInt { min: i64, max: i64 },
    RandomFloat { min: f64, max: f64 },

    Latitude,
    Longitude,
    City,
    Country,
    PostalCode,
    StreetAddress,

    DatetimePast,
    DatetimeRecent,
    DateFuture,

    HexColor,
    IPv4,
    CurrencyCode,
    CountryCode,
    EnumPick { values: Vec<String> },
    JsonEmpty,
    BoolRandom,

    RandomString { max_length: u32 },
    Null,
    Skip,
}

pub fn detect_generator(column: &Column, enums: &[EnumType]) -> GeneratorType {
    if column.is_generated || column.is_identity {
        return GeneratorType::Skip;
    }

    if let DataType::Enum(enum_name) = &column.data_type {
        if let Some(e) = enums.iter().find(|e| &e.name == enum_name) {
            return GeneratorType::EnumPick {
                values: e.values.clone(),
            };
        }
    }

    let name = column.name.to_ascii_lowercase();

    if let Some(g) = match_by_name(&name, column) {
        return g;
    }

    fallback_by_type(column)
}

fn match_by_name(name: &str, column: &Column) -> Option<GeneratorType> {
    if name.contains("email") {
        return Some(GeneratorType::Email);
    }
    if name.contains("first_name") || name.contains("firstname") {
        return Some(GeneratorType::FirstName);
    }
    if name.contains("last_name") || name.contains("lastname") {
        return Some(GeneratorType::LastName);
    }
    if name == "name" || name.contains("full_name") {
        return Some(GeneratorType::FullName);
    }
    if name.contains("username") || name.contains("user_name") {
        return Some(GeneratorType::Username);
    }
    if name.contains("phone") || name.contains("mobile") || name.contains("tel") {
        return Some(GeneratorType::Phone);
    }
    if name.contains("avatar") {
        return Some(GeneratorType::AvatarUrl);
    }
    if name.contains("url") || name.contains("link") || name.contains("website") {
        return Some(GeneratorType::Url);
    }
    if name.contains("password") || name.contains("passwd") {
        return Some(GeneratorType::Password);
    }
    if name.contains("slug") {
        return Some(GeneratorType::Slug);
    }
    if name.contains("title") || name.contains("subject") {
        return Some(GeneratorType::Sentence);
    }
    if name.contains("description")
        || name.contains("bio")
        || name.contains("about")
        || name.contains("body")
        || name.contains("content")
    {
        return Some(GeneratorType::Paragraph);
    }
    if name.contains("company") || name.contains("organization") {
        return Some(GeneratorType::CompanyName);
    }
    if name.contains("uuid") {
        return Some(GeneratorType::Uuid);
    }
    if name.contains("token") || name.contains("secret") || name.contains("api_key") {
        return Some(GeneratorType::Token);
    }
    if name.contains("sku") || (name.contains("code") && column.max_length.is_some_and(|l| l <= 50))
    {
        return Some(GeneratorType::Sku);
    }
    if name.contains("price")
        || name.contains("amount")
        || name.contains("cost")
        || name.contains("total")
        || name.contains("fee")
    {
        return Some(GeneratorType::Money {
            min: 0.01,
            max: 10_000.0,
        });
    }
    if name.contains("latitude") || name.contains("lat") {
        return Some(GeneratorType::Latitude);
    }
    if name.contains("longitude") || name.contains("lng") || name.contains("lon") {
        return Some(GeneratorType::Longitude);
    }
    if name.contains("city") {
        return Some(GeneratorType::City);
    }
    if name.contains("country_code") {
        return Some(GeneratorType::CountryCode);
    }
    if name.contains("country") {
        return Some(GeneratorType::Country);
    }
    if name.contains("zip") || name.contains("postal") {
        return Some(GeneratorType::PostalCode);
    }
    if name.contains("address") && !name.contains("email") && !name.contains("ip") {
        return Some(GeneratorType::StreetAddress);
    }
    if name.contains("color") || name.contains("colour") {
        return Some(GeneratorType::HexColor);
    }
    if name.contains("ip") && matches!(column.data_type, DataType::Varchar | DataType::Char) {
        return Some(GeneratorType::IPv4);
    }
    if name.contains("currency") {
        return Some(GeneratorType::CurrencyCode);
    }
    None
}

fn fallback_by_type(column: &Column) -> GeneratorType {
    match &column.data_type {
        DataType::Boolean => GeneratorType::BoolRandom,

        DataType::SmallInt | DataType::Integer | DataType::BigInt => GeneratorType::RandomInt {
            min: 1,
            max: 10_000,
        },

        DataType::Real | DataType::DoublePrecision => GeneratorType::RandomFloat {
            min: 0.0,
            max: 1_000.0,
        },

        DataType::Numeric => {
            if column.numeric_scale == Some(2) {
                GeneratorType::Money {
                    min: 0.01,
                    max: 10_000.0,
                }
            } else {
                GeneratorType::RandomFloat {
                    min: 0.0,
                    max: 1_000.0,
                }
            }
        }

        DataType::Char | DataType::Varchar => GeneratorType::RandomString {
            max_length: column.max_length.unwrap_or(255),
        },

        DataType::Text => GeneratorType::Paragraph,

        DataType::Date | DataType::Timestamp | DataType::TimestampTz => GeneratorType::DatetimePast,

        DataType::Uuid => GeneratorType::Uuid,

        DataType::Json | DataType::Jsonb => GeneratorType::JsonEmpty,

        DataType::Inet | DataType::Cidr => GeneratorType::IPv4,

        DataType::Money => GeneratorType::Money {
            min: 0.01,
            max: 10_000.0,
        },

        DataType::Time
        | DataType::TimeTz
        | DataType::Interval
        | DataType::Bytea
        | DataType::MacAddr
        | DataType::Array(_)
        | DataType::Enum(_)
        | DataType::Other(_) => GeneratorType::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(name: &str, dt: DataType) -> Column {
        Column {
            name: name.into(),
            data_type: dt,
            is_nullable: true,
            is_identity: false,
            is_generated: false,
            default_value: None,
            max_length: None,
            numeric_precision: None,
            numeric_scale: None,
        }
    }

    fn col_with_len(name: &str, dt: DataType, max_length: u32) -> Column {
        let mut c = col(name, dt);
        c.max_length = Some(max_length);
        c
    }

    fn detect(name: &str, dt: DataType) -> GeneratorType {
        detect_generator(&col(name, dt), &[])
    }

    // --- Skip rules ----------------------------------------------------------

    #[test]
    fn test_semantic_skips_generated_column() {
        let mut c = col("computed_total", DataType::Integer);
        c.is_generated = true;
        assert_eq!(detect_generator(&c, &[]), GeneratorType::Skip);
    }

    #[test]
    fn test_semantic_skips_identity_column() {
        let mut c = col("id", DataType::Integer);
        c.is_identity = true;
        assert_eq!(detect_generator(&c, &[]), GeneratorType::Skip);
    }

    // --- Enum ----------------------------------------------------------------

    #[test]
    fn test_semantic_picks_known_enum() {
        let c = col("status", DataType::Enum("order_status".into()));
        let enums = vec![EnumType {
            name: "order_status".into(),
            values: vec!["pending".into(), "shipped".into(), "delivered".into()],
        }];
        assert_eq!(
            detect_generator(&c, &enums),
            GeneratorType::EnumPick {
                values: vec!["pending".into(), "shipped".into(), "delivered".into()]
            }
        );
    }

    #[test]
    fn test_semantic_unknown_enum_falls_through_to_null() {
        let c = col("status", DataType::Enum("not_registered".into()));
        assert_eq!(detect_generator(&c, &[]), GeneratorType::Null);
    }

    // --- Text patterns -------------------------------------------------------

    #[test]
    fn test_semantic_detects_email() {
        assert_eq!(detect("email", DataType::Varchar), GeneratorType::Email);
        assert_eq!(
            detect("user_email", DataType::Varchar),
            GeneratorType::Email
        );
        assert_eq!(
            detect("EMAIL_ADDRESS", DataType::Varchar),
            GeneratorType::Email
        );
    }

    #[test]
    fn test_semantic_detects_first_name_with_underscore() {
        assert_eq!(
            detect("first_name", DataType::Varchar),
            GeneratorType::FirstName
        );
    }

    #[test]
    fn test_semantic_detects_first_name_without_underscore() {
        assert_eq!(
            detect("firstname", DataType::Varchar),
            GeneratorType::FirstName
        );
    }

    #[test]
    fn test_semantic_detects_last_name() {
        assert_eq!(
            detect("last_name", DataType::Varchar),
            GeneratorType::LastName
        );
        assert_eq!(
            detect("lastname", DataType::Varchar),
            GeneratorType::LastName
        );
    }

    #[test]
    fn test_semantic_detects_full_name_equals_name() {
        assert_eq!(detect("name", DataType::Varchar), GeneratorType::FullName);
    }

    #[test]
    fn test_semantic_detects_full_name_substring() {
        assert_eq!(
            detect("full_name", DataType::Varchar),
            GeneratorType::FullName
        );
    }

    #[test]
    fn test_semantic_detects_username() {
        assert_eq!(
            detect("username", DataType::Varchar),
            GeneratorType::Username
        );
        assert_eq!(
            detect("user_name", DataType::Varchar),
            GeneratorType::Username
        );
    }

    #[test]
    fn test_semantic_detects_phone() {
        assert_eq!(detect("phone", DataType::Varchar), GeneratorType::Phone);
        assert_eq!(
            detect("phone_number", DataType::Varchar),
            GeneratorType::Phone
        );
        assert_eq!(detect("mobile", DataType::Varchar), GeneratorType::Phone);
        assert_eq!(detect("tel", DataType::Varchar), GeneratorType::Phone);
    }

    #[test]
    fn test_semantic_detects_avatar_before_url() {
        assert_eq!(
            detect("avatar_url", DataType::Varchar),
            GeneratorType::AvatarUrl
        );
        assert_eq!(
            detect("avatar", DataType::Varchar),
            GeneratorType::AvatarUrl
        );
    }

    #[test]
    fn test_semantic_detects_url() {
        assert_eq!(detect("url", DataType::Varchar), GeneratorType::Url);
        assert_eq!(
            detect("homepage_link", DataType::Varchar),
            GeneratorType::Url
        );
        assert_eq!(detect("website", DataType::Varchar), GeneratorType::Url);
    }

    #[test]
    fn test_semantic_detects_password() {
        assert_eq!(
            detect("password", DataType::Varchar),
            GeneratorType::Password
        );
        assert_eq!(detect("passwd", DataType::Varchar), GeneratorType::Password);
        assert_eq!(
            detect("password_hash", DataType::Varchar),
            GeneratorType::Password
        );
    }

    #[test]
    fn test_semantic_detects_slug() {
        assert_eq!(detect("slug", DataType::Varchar), GeneratorType::Slug);
    }

    #[test]
    fn test_semantic_detects_title_as_sentence() {
        assert_eq!(detect("title", DataType::Varchar), GeneratorType::Sentence);
        assert_eq!(
            detect("subject", DataType::Varchar),
            GeneratorType::Sentence
        );
    }

    #[test]
    fn test_semantic_detects_description_as_paragraph() {
        assert_eq!(
            detect("description", DataType::Text),
            GeneratorType::Paragraph
        );
        assert_eq!(detect("bio", DataType::Text), GeneratorType::Paragraph);
        assert_eq!(detect("about", DataType::Text), GeneratorType::Paragraph);
        assert_eq!(detect("body", DataType::Text), GeneratorType::Paragraph);
        assert_eq!(detect("content", DataType::Text), GeneratorType::Paragraph);
    }

    #[test]
    fn test_semantic_detects_company() {
        assert_eq!(
            detect("company", DataType::Varchar),
            GeneratorType::CompanyName
        );
        assert_eq!(
            detect("company_name", DataType::Varchar),
            GeneratorType::CompanyName
        );
        assert_eq!(
            detect("organization", DataType::Varchar),
            GeneratorType::CompanyName
        );
    }

    // --- Identifiers ---------------------------------------------------------

    #[test]
    fn test_semantic_detects_uuid_by_name() {
        assert_eq!(
            detect("session_uuid", DataType::Varchar),
            GeneratorType::Uuid
        );
    }

    #[test]
    fn test_semantic_detects_token() {
        assert_eq!(
            detect("auth_token", DataType::Varchar),
            GeneratorType::Token
        );
        assert_eq!(detect("secret", DataType::Varchar), GeneratorType::Token);
        assert_eq!(detect("api_key", DataType::Varchar), GeneratorType::Token);
    }

    #[test]
    fn test_semantic_detects_sku() {
        assert_eq!(detect("sku", DataType::Varchar), GeneratorType::Sku);
        assert_eq!(detect("product_sku", DataType::Varchar), GeneratorType::Sku);
    }

    #[test]
    fn test_semantic_short_code_is_sku() {
        let c = col_with_len("product_code", DataType::Varchar, 20);
        assert_eq!(detect_generator(&c, &[]), GeneratorType::Sku);
    }

    #[test]
    fn test_semantic_long_code_is_not_sku() {
        // code with max_length > 50 falls through name rules; varchar(200) → RandomString(200).
        let c = col_with_len("product_code", DataType::Varchar, 200);
        assert_eq!(
            detect_generator(&c, &[]),
            GeneratorType::RandomString { max_length: 200 }
        );
    }

    // --- Money / numeric -----------------------------------------------------

    #[test]
    fn test_semantic_detects_price_as_money() {
        assert!(matches!(
            detect("price", DataType::Numeric),
            GeneratorType::Money { .. }
        ));
        assert!(matches!(
            detect("unit_price", DataType::Numeric),
            GeneratorType::Money { .. }
        ));
    }

    #[test]
    fn test_semantic_detects_amount_cost_total_fee_as_money() {
        for name in ["amount", "total_cost", "shipping_fee", "subtotal"] {
            assert!(
                matches!(detect(name, DataType::Numeric), GeneratorType::Money { .. }),
                "{name} should be Money"
            );
        }
    }

    // --- Geo -----------------------------------------------------------------

    #[test]
    fn test_semantic_detects_latitude() {
        assert_eq!(
            detect("latitude", DataType::DoublePrecision),
            GeneratorType::Latitude
        );
        assert_eq!(
            detect("lat", DataType::DoublePrecision),
            GeneratorType::Latitude
        );
    }

    #[test]
    fn test_semantic_detects_longitude() {
        assert_eq!(
            detect("longitude", DataType::DoublePrecision),
            GeneratorType::Longitude
        );
        assert_eq!(
            detect("lng", DataType::DoublePrecision),
            GeneratorType::Longitude
        );
        assert_eq!(
            detect("lon", DataType::DoublePrecision),
            GeneratorType::Longitude
        );
    }

    #[test]
    fn test_semantic_detects_city() {
        assert_eq!(detect("city", DataType::Varchar), GeneratorType::City);
        assert_eq!(
            detect("billing_city", DataType::Varchar),
            GeneratorType::City
        );
    }

    #[test]
    fn test_semantic_country_code_wins_over_country() {
        assert_eq!(
            detect("country_code", DataType::Varchar),
            GeneratorType::CountryCode
        );
    }

    #[test]
    fn test_semantic_detects_country() {
        assert_eq!(detect("country", DataType::Varchar), GeneratorType::Country);
    }

    #[test]
    fn test_semantic_detects_postal_code() {
        assert_eq!(
            detect("postal_code", DataType::Varchar),
            GeneratorType::PostalCode
        );
        assert_eq!(detect("zip", DataType::Varchar), GeneratorType::PostalCode);
        assert_eq!(
            detect("zipcode", DataType::Varchar),
            GeneratorType::PostalCode
        );
    }

    #[test]
    fn test_semantic_detects_street_address() {
        assert_eq!(
            detect("street_address", DataType::Varchar),
            GeneratorType::StreetAddress
        );
        assert_eq!(
            detect("address", DataType::Varchar),
            GeneratorType::StreetAddress
        );
    }

    #[test]
    fn test_semantic_email_address_is_email_not_address() {
        assert_eq!(
            detect("email_address", DataType::Varchar),
            GeneratorType::Email
        );
    }

    // --- Structured ----------------------------------------------------------

    #[test]
    fn test_semantic_detects_color() {
        assert_eq!(detect("color", DataType::Varchar), GeneratorType::HexColor);
        assert_eq!(
            detect("background_colour", DataType::Varchar),
            GeneratorType::HexColor
        );
    }

    #[test]
    fn test_semantic_detects_ip_in_varchar() {
        assert_eq!(detect("ip_address", DataType::Varchar), GeneratorType::IPv4);
    }

    #[test]
    fn test_semantic_ip_in_non_varchar_falls_through() {
        // "ip" in a TEXT column doesn't trigger IPv4 (rule requires varchar/char).
        // TEXT then falls through name rules to its type fallback → Paragraph.
        assert_eq!(detect("ip_log", DataType::Text), GeneratorType::Paragraph);
    }

    #[test]
    fn test_semantic_detects_currency() {
        assert_eq!(
            detect("currency", DataType::Varchar),
            GeneratorType::CurrencyCode
        );
        assert_eq!(
            detect("currency_code", DataType::Varchar),
            GeneratorType::CurrencyCode
        );
    }

    // --- Type-based fallbacks ------------------------------------------------

    #[test]
    fn test_fallback_boolean() {
        assert_eq!(detect("flag", DataType::Boolean), GeneratorType::BoolRandom);
    }

    #[test]
    fn test_fallback_integer() {
        assert!(matches!(
            detect("count", DataType::Integer),
            GeneratorType::RandomInt { .. }
        ));
        assert!(matches!(
            detect("quantity", DataType::BigInt),
            GeneratorType::RandomInt { .. }
        ));
    }

    #[test]
    fn test_fallback_float() {
        assert!(matches!(
            detect("ratio", DataType::DoublePrecision),
            GeneratorType::RandomFloat { .. }
        ));
    }

    #[test]
    fn test_fallback_numeric_scale_two_is_money() {
        let mut c = col("balance", DataType::Numeric);
        c.numeric_scale = Some(2);
        assert!(matches!(
            detect_generator(&c, &[]),
            GeneratorType::Money { .. }
        ));
    }

    #[test]
    fn test_fallback_numeric_without_scale_is_random_float() {
        let c = col("ratio", DataType::Numeric);
        assert!(matches!(
            detect_generator(&c, &[]),
            GeneratorType::RandomFloat { .. }
        ));
    }

    #[test]
    fn test_fallback_varchar_uses_max_length() {
        let c = col_with_len("misc", DataType::Varchar, 64);
        assert_eq!(
            detect_generator(&c, &[]),
            GeneratorType::RandomString { max_length: 64 }
        );
    }

    #[test]
    fn test_fallback_varchar_without_length_uses_255() {
        let c = col("misc", DataType::Varchar);
        assert_eq!(
            detect_generator(&c, &[]),
            GeneratorType::RandomString { max_length: 255 }
        );
    }

    #[test]
    fn test_fallback_text_is_paragraph() {
        assert_eq!(detect("notes", DataType::Text), GeneratorType::Paragraph);
    }

    #[test]
    fn test_fallback_timestamp_is_datetime_past() {
        assert_eq!(
            detect("created_when", DataType::Timestamp),
            GeneratorType::DatetimePast
        );
        assert_eq!(
            detect("when", DataType::TimestampTz),
            GeneratorType::DatetimePast
        );
    }

    #[test]
    fn test_fallback_date_is_datetime_past() {
        assert_eq!(detect("when", DataType::Date), GeneratorType::DatetimePast);
    }

    #[test]
    fn test_fallback_jsonb_is_json_empty() {
        assert_eq!(
            detect("metadata", DataType::Jsonb),
            GeneratorType::JsonEmpty
        );
        assert_eq!(detect("payload", DataType::Json), GeneratorType::JsonEmpty);
    }

    #[test]
    fn test_fallback_uuid_type_is_uuid() {
        assert_eq!(detect("anything", DataType::Uuid), GeneratorType::Uuid);
    }

    #[test]
    fn test_fallback_inet_is_ipv4() {
        assert_eq!(detect("source", DataType::Inet), GeneratorType::IPv4);
    }

    #[test]
    fn test_fallback_money_type_is_money() {
        assert!(matches!(
            detect("anything", DataType::Money),
            GeneratorType::Money { .. }
        ));
    }

    #[test]
    fn test_fallback_unsupported_type_is_null() {
        assert_eq!(
            detect("data", DataType::Array(Box::new(DataType::Integer))),
            GeneratorType::Null
        );
        assert_eq!(
            detect("data", DataType::Other("tsvector".into())),
            GeneratorType::Null
        );
        assert_eq!(detect("data", DataType::Interval), GeneratorType::Null);
    }

    // --- Case insensitivity --------------------------------------------------

    #[test]
    fn test_semantic_is_case_insensitive() {
        assert_eq!(detect("EMAIL", DataType::Varchar), GeneratorType::Email);
        assert_eq!(
            detect("First_Name", DataType::Varchar),
            GeneratorType::FirstName
        );
        assert_eq!(
            detect("PASSWORD", DataType::Varchar),
            GeneratorType::Password
        );
    }
}
