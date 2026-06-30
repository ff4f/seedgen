//! Error type for the profiling module.

/// Errors that can occur while profiling a database or applying a profile.
#[derive(thiserror::Error, Debug)]
pub enum ProfileError {
    #[error("Database query failed: {0}")]
    QueryFailed(#[from] sqlx::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Connected as superuser — use a read-only role (override with --allow-superuser)")]
    SuperuserNotAllowed,

    #[error("Statement timeout exceeded for query on table '{table}', column '{column}'")]
    StatementTimeout { table: String, column: String },

    #[error("Profile YAML parse error: {0}")]
    ParseError(#[from] serde_yaml::Error),

    #[error("Profile JSON error: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("Profile version '{found}' not supported (expected '{expected}')")]
    UnsupportedVersion { found: String, expected: String },

    #[error("Import format error: {0}")]
    ImportFormatError(String),

    #[error("Column '{table}.{column}' — cardinality query returned unexpected result")]
    CardinalityError { table: String, column: String },

    #[error("Scale factor must be between 0.0 (exclusive) and 1.0 (inclusive), got {0}")]
    InvalidScale(f64),

    #[error("No tables found in database to profile")]
    EmptySchema,
}
