//! Sensitive-column detection and explicit include/exclude helpers.
//!
//! Detection is purely name-based: a column is sensitive if its lower-cased name
//! *contains* any [`SENSITIVE_PATTERNS`] substring. This mirrors the existing
//! semantic name-matching approach and is intentionally conservative — when in
//! doubt, skip.

/// Column-name substrings that mark a column as holding sensitive data. A column
/// whose lower-cased name contains any of these is auto-skipped during profiling
/// (no statistics captured).
pub const SENSITIVE_PATTERNS: &[&str] = &[
    "password",
    "passwd",
    "pwd",
    "secret",
    "token",
    "api_key",
    "apikey",
    "access_key",
    "private_key",
    "ssn",
    "social_security",
    "tax_id",
    "national_id",
    "credit_card",
    "card_number",
    "cvv",
    "cvc",
    "bank_account",
    "iban",
    "routing_number",
    "pin",
    "otp",
    "mfa",
    "totp",
];

/// Whether `column_name` matches any built-in sensitive pattern.
pub fn is_sensitive_column(column_name: &str) -> bool {
    let lower = column_name.to_lowercase();
    SENSITIVE_PATTERNS.iter().any(|p| lower.contains(p))
}

/// Whether `table.column` appears in a `"table.column"`-formatted list.
fn matches_qualified(table: &str, column: &str, list: &[String]) -> bool {
    let qualified = format!("{table}.{column}");
    list.iter().any(|c| c == &qualified)
}

/// Whether `table.column` appears in an explicit exclude list (entries are in
/// `"table.column"` form).
pub fn is_excluded(table: &str, column: &str, exclude_columns: &[String]) -> bool {
    matches_qualified(table, column, exclude_columns)
}

/// Whether `table.column` appears in an explicit include list (entries are in
/// `"table.column"` form). Used to override sensitive-pattern auto-skipping.
pub fn is_included(table: &str, column: &str, include_columns: &[String]) -> bool {
    matches_qualified(table, column, include_columns)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sensitive_detects_password_variants() {
        assert!(is_sensitive_column("password"));
        assert!(is_sensitive_column("password_hash"));
        assert!(is_sensitive_column("user_password"));
        assert!(is_sensitive_column("passwd"));
        assert!(is_sensitive_column("PWD")); // case-insensitive
    }

    #[test]
    fn test_sensitive_detects_financial_and_pii() {
        assert!(is_sensitive_column("ssn"));
        assert!(is_sensitive_column("social_security"));
        assert!(is_sensitive_column("tax_id"));
        assert!(is_sensitive_column("credit_card"));
        assert!(is_sensitive_column("card_number"));
        assert!(is_sensitive_column("cvv"));
        assert!(is_sensitive_column("iban"));
        assert!(is_sensitive_column("api_key"));
        assert!(is_sensitive_column("reset_token"));
    }

    #[test]
    fn test_sensitive_does_not_flag_normal_columns() {
        assert!(!is_sensitive_column("status"));
        assert!(!is_sensitive_column("email"));
        assert!(!is_sensitive_column("price"));
        assert!(!is_sensitive_column("created_at"));
        assert!(!is_sensitive_column("role"));
        assert!(!is_sensitive_column("name"));
        assert!(!is_sensitive_column("total_amount"));
        assert!(!is_sensitive_column("passport_number")); // "password" pattern, not "pass"
    }

    #[test]
    fn test_is_excluded_matches_qualified_name() {
        let exclude = vec![
            "users.internal_notes".to_string(),
            "orders.admin_comment".to_string(),
        ];
        assert!(is_excluded("users", "internal_notes", &exclude));
        assert!(is_excluded("orders", "admin_comment", &exclude));
        assert!(!is_excluded("users", "email", &exclude));
        // Same column name but different table must NOT match.
        assert!(!is_excluded("posts", "internal_notes", &exclude));
        // Empty exclude list never matches.
        assert!(!is_excluded("users", "internal_notes", &[]));
    }

    #[test]
    fn test_is_included_matches_qualified_name() {
        let include = vec!["orders.token_type".to_string()];
        assert!(is_included("orders", "token_type", &include));
        assert!(!is_included("users", "token_type", &include));
        assert!(!is_included("orders", "token_type", &[]));
    }
}
