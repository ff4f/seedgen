//! Audit log for profiling: every executed query and every skipped column is
//! recorded, then written to `.seedgen-profile-audit.log`.
//!
//! Entries hold the query SQL (which never contains data values) and a short
//! result *summary* (aggregate counts, never row-level values), so the log is
//! safe to share and review.

use std::fs;
use std::path::Path;

use chrono::{DateTime, Utc};

use crate::profile::errors::ProfileError;

/// What an [`AuditEntry`] records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditAction {
    /// A query was executed. `result_summary` is an aggregate description only.
    Query { result_summary: String },
    /// A column was skipped (sensitive / excluded / serial / unsupported).
    Skip { reason: String },
}

/// One audited action.
#[derive(Debug, Clone)]
pub struct AuditEntry {
    pub timestamp: DateTime<Utc>,
    pub action: AuditAction,
    /// For a query, the SQL text. For a skip, the `"table"."column"` subject.
    pub subject: String,
    pub duration_ms: u64,
}

/// An ordered log of profiling actions.
#[derive(Debug, Clone, Default)]
pub struct AuditLog {
    entries: Vec<AuditEntry>,
}

impl AuditLog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record an executed query with its duration and a (data-free) summary.
    pub fn record_query(&mut self, sql: &str, duration_ms: u64, result_summary: impl Into<String>) {
        self.entries.push(AuditEntry {
            timestamp: Utc::now(),
            action: AuditAction::Query {
                result_summary: result_summary.into(),
            },
            subject: sql.to_string(),
            duration_ms,
        });
    }

    /// Record a skipped column with a human-readable reason.
    pub fn record_skip(&mut self, subject: &str, reason: impl Into<String>) {
        self.entries.push(AuditEntry {
            timestamp: Utc::now(),
            action: AuditAction::Skip {
                reason: reason.into(),
            },
            subject: subject.to_string(),
            duration_ms: 0,
        });
    }

    pub fn entries(&self) -> &[AuditEntry] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Number of `Query` entries.
    pub fn query_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| matches!(e.action, AuditAction::Query { .. }))
            .count()
    }

    /// Number of `Skip` entries.
    pub fn skip_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| matches!(e.action, AuditAction::Skip { .. }))
            .count()
    }

    /// Render the log as plain text, one line per entry.
    pub fn render(&self) -> String {
        let mut out = String::new();
        for e in &self.entries {
            let ts = e.timestamp.to_rfc3339();
            match &e.action {
                AuditAction::Query { result_summary } => {
                    out.push_str(&format!(
                        "{ts} QUERY  {}  → {result_summary}  ({}ms)\n",
                        e.subject, e.duration_ms
                    ));
                }
                AuditAction::Skip { reason } => {
                    out.push_str(&format!("{ts} SKIP   {} — {reason}\n", e.subject));
                }
            }
        }
        out
    }

    /// Write the rendered log to `path` (e.g. `.seedgen-profile-audit.log`).
    pub fn write_to_file(&self, path: &Path) -> Result<(), ProfileError> {
        fs::write(path, self.render())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audit_log_records_queries_and_skips() {
        let mut log = AuditLog::new();
        assert!(log.is_empty());
        log.record_query("SELECT COUNT(*) FROM \"users\"", 1, "100 rows");
        log.record_skip(
            "\"users\".\"password_hash\"",
            "sensitive column (pattern: password)",
        );

        assert_eq!(log.len(), 2);
        assert_eq!(log.query_count(), 1);
        assert_eq!(log.skip_count(), 1);
    }

    #[test]
    fn test_audit_render_contains_query_and_skip_lines() {
        let mut log = AuditLog::new();
        log.record_query("SELECT COUNT(*) FROM \"users\"", 2, "100 rows");
        log.record_skip(
            "\"users\".\"reset_token\"",
            "sensitive column (pattern: token)",
        );

        let text = log.render();
        assert!(text.contains("QUERY"));
        assert!(text.contains("SELECT COUNT(*) FROM \"users\""));
        assert!(text.contains("→ 100 rows"));
        assert!(text.contains("SKIP"));
        assert!(text.contains("reset_token"));
        assert!(text.contains("pattern: token"));
    }

    #[test]
    fn test_audit_write_to_file_roundtrips() {
        let mut log = AuditLog::new();
        log.record_query("SELECT COUNT(*) FROM \"orders\"", 3, "354281 rows");

        let dir = std::env::temp_dir();
        let path = dir.join(format!("seedgen-audit-test-{}.log", std::process::id()));
        log.write_to_file(&path).expect("write audit log");
        let read = std::fs::read_to_string(&path).expect("read audit log");
        assert!(read.contains("354281 rows"));
        let _ = std::fs::remove_file(&path);
    }
}
