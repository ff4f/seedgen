//! Profiling options and their serializable summary.

use serde::{Deserialize, Serialize};

use crate::profile::sensitive::SENSITIVE_PATTERNS;

/// Options controlling how profiling is performed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProfileOptions {
    /// Max distinct values before a column is treated as high-cardinality (and
    /// its individual values are NOT captured). Default: 50.
    pub cardinality_threshold: usize,

    /// Column-name substrings that mark a column as sensitive (auto-skipped).
    /// Default: [`default_sensitive_patterns`].
    pub sensitive_patterns: Vec<String>,

    /// Additional columns to explicitly skip, in `table.column` form.
    pub exclude_columns: Vec<String>,

    /// Columns to explicitly include even if flagged sensitive, `table.column`.
    pub include_columns: Vec<String>,

    /// Statement timeout (seconds) applied to every profiling query. Default: 30.
    pub statement_timeout_secs: u32,

    /// Refuse to profile when connected as a superuser.
    pub strict_security: bool,

    /// Capture hourly density for timestamp columns.
    pub capture_hourly: bool,

    /// Capture monthly density for timestamp columns.
    pub capture_monthly: bool,

    /// Capture extended percentiles for numeric columns.
    pub capture_percentiles: bool,
}

impl Default for ProfileOptions {
    fn default() -> Self {
        Self {
            cardinality_threshold: 50,
            sensitive_patterns: default_sensitive_patterns(),
            exclude_columns: Vec::new(),
            include_columns: Vec::new(),
            statement_timeout_secs: 30,
            strict_security: false,
            capture_hourly: true,
            capture_monthly: true,
            capture_percentiles: true,
        }
    }
}

/// The built-in sensitive-pattern list as owned `String`s, for [`ProfileOptions`].
pub fn default_sensitive_patterns() -> Vec<String> {
    SENSITIVE_PATTERNS
        .iter()
        .map(|s| (*s).to_string())
        .collect()
}

/// A compact, serializable summary of the options actually used for a profile.
/// Embedded in [`DatabaseProfile`](crate::profile::stats::DatabaseProfile).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProfileOptionsSummary {
    pub cardinality_threshold: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped_sensitive: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_profile_options_default_values() {
        let o = ProfileOptions::default();
        assert_eq!(o.cardinality_threshold, 50);
        assert_eq!(o.statement_timeout_secs, 30);
        assert!(!o.strict_security);
        assert!(o.capture_hourly);
        assert!(o.capture_monthly);
        assert!(o.capture_percentiles);
        assert!(o.exclude_columns.is_empty());
        assert!(o.include_columns.is_empty());
        assert!(!o.sensitive_patterns.is_empty());
        assert!(o.sensitive_patterns.iter().any(|p| p == "password"));
    }

    #[test]
    fn test_default_sensitive_patterns_matches_const() {
        assert_eq!(default_sensitive_patterns().len(), SENSITIVE_PATTERNS.len());
        assert!(default_sensitive_patterns().contains(&"ssn".to_string()));
    }

    #[test]
    fn test_profile_options_serde_roundtrip() {
        let o = ProfileOptions::default();
        let yaml = serde_yaml::to_string(&o).unwrap();
        let parsed: ProfileOptions = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(o, parsed);
    }
}
