//! Production statistics profiling.
//!
//! Reads *only* aggregate statistics from a database (never row-level data) and
//! represents them as a serializable [`DatabaseProfile`]. A profile can later be
//! converted into scenario overrides that drive the existing generation engine.
//!
//! This module is purely additive: nothing here changes introspection, the
//! resolver, semantic detection, generators, output, or lifecycle.

pub mod applicator;
pub mod audit;
pub mod collector;
pub mod compliance;
pub mod config;
pub mod errors;
pub mod offline;
pub mod output;
pub mod queries;
pub mod sensitive;
pub mod stats;

pub use applicator::ProfileApplicator;
pub use audit::{AuditAction, AuditEntry, AuditLog};
pub use collector::ProfileCollector;
pub use compliance::{ComplianceCheck, ComplianceReport, ComplianceValidator};
pub use config::{default_sensitive_patterns, ProfileOptions, ProfileOptionsSummary};
pub use errors::ProfileError;
pub use offline::{export_collection_sql, import_results};
pub use queries::{PlannedQuery, QueryBuilder, QueryKind};
pub use sensitive::{is_excluded, is_included, is_sensitive_column, SENSITIVE_PATTERNS};
pub use stats::{ColumnProfile, DatabaseProfile, ParentRatio, Percentiles, TableProfile};
