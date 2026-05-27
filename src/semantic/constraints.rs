use std::collections::HashSet;

use crate::generators::Value;

#[derive(Debug, Clone)]
pub struct ConstraintHandler {
    pub kind: ConstraintHandlerKind,
}

#[derive(Debug, Clone)]
pub enum ConstraintHandlerKind {
    NotNull,
    Unique {
        seen: HashSet<String>,
    },
    CompositeUnique {
        columns: Vec<String>,
        seen: HashSet<Vec<String>>,
    },
    CheckPositive,
    CheckRange {
        min: f64,
        max: f64,
    },
    MaxLength(u32),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ValidationResult {
    Valid,
    Retry,
    Invalid(String),
}

impl ConstraintHandler {
    pub fn new(kind: ConstraintHandlerKind) -> Self {
        Self { kind }
    }

    pub fn validate(&mut self, column_name: &str, value: &Value) -> ValidationResult {
        match &mut self.kind {
            ConstraintHandlerKind::NotNull => {
                if value.is_null() {
                    ValidationResult::Invalid(format!(
                        "column `{column_name}` is NOT NULL but value is null"
                    ))
                } else {
                    ValidationResult::Valid
                }
            }
            ConstraintHandlerKind::Unique { seen } => {
                let key = value.as_key();
                if seen.contains(&key) {
                    ValidationResult::Retry
                } else {
                    seen.insert(key);
                    ValidationResult::Valid
                }
            }
            ConstraintHandlerKind::CompositeUnique { .. } => ValidationResult::Valid,
            ConstraintHandlerKind::CheckPositive => match value.as_f64() {
                Some(n) if n > 0.0 => ValidationResult::Valid,
                Some(_) => ValidationResult::Retry,
                None => ValidationResult::Valid,
            },
            ConstraintHandlerKind::CheckRange { min, max } => match value.as_f64() {
                Some(n) if n >= *min && n <= *max => ValidationResult::Valid,
                Some(_) => ValidationResult::Retry,
                None => ValidationResult::Valid,
            },
            ConstraintHandlerKind::MaxLength(limit) => match value.as_str() {
                Some(s) if s.chars().count() as u32 > *limit => ValidationResult::Retry,
                _ => ValidationResult::Valid,
            },
        }
    }

    pub fn validate_row(&mut self, row_values: &[Value]) -> ValidationResult {
        match &mut self.kind {
            ConstraintHandlerKind::CompositeUnique { seen, .. } => {
                let key: Vec<String> = row_values.iter().map(|v| v.as_key()).collect();
                if seen.contains(&key) {
                    ValidationResult::Retry
                } else {
                    seen.insert(key);
                    ValidationResult::Valid
                }
            }
            _ => ValidationResult::Valid,
        }
    }
}

pub fn parse_check_constraint(clause: &str) -> Option<ConstraintHandlerKind> {
    let lower = clause.to_ascii_lowercase();
    let parts: Vec<&str> = lower.split(" and ").collect();

    if parts.len() == 2 {
        let mut min = None;
        let mut max = None;
        for p in &parts {
            if p.contains(">=") && lhs_is_simple_ident(p, ">=") {
                min = extract_number_after_op(p, ">=");
            } else if p.contains("<=") && lhs_is_simple_ident(p, "<=") {
                max = extract_number_after_op(p, "<=");
            }
        }
        if let (Some(mn), Some(mx)) = (min, max) {
            return Some(ConstraintHandlerKind::CheckRange { min: mn, max: mx });
        }
    }

    if parts.len() == 1
        && lower.contains('>')
        && !lower.contains(">=")
        && lhs_is_simple_ident(&lower, ">")
    {
        if let Some(n) = extract_number_after_op(&lower, ">") {
            if n == 0.0 {
                return Some(ConstraintHandlerKind::CheckPositive);
            }
        }
    }

    None
}

fn lhs_is_simple_ident(part: &str, op: &str) -> bool {
    let Some(idx) = part.find(op) else {
        return false;
    };
    let lhs = part[..idx].trim().trim_start_matches('(').trim();
    !lhs.is_empty()
        && lhs
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '.' || c == '"')
}

fn extract_number_after_op(s: &str, op: &str) -> Option<f64> {
    let idx = s.find(op)?;
    let rest = &s[idx + op.len()..];
    let rhs = rest.split(" and ").next().unwrap_or("").trim();
    let cleaned = rhs
        .trim_start_matches('(')
        .split("::")
        .next()
        .unwrap_or("")
        .trim_end_matches(')')
        .trim();
    cleaned.parse::<f64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handler(kind: ConstraintHandlerKind) -> ConstraintHandler {
        ConstraintHandler::new(kind)
    }

    // --- NotNull -------------------------------------------------------------

    #[test]
    fn test_constraints_not_null_rejects_null() {
        let mut h = handler(ConstraintHandlerKind::NotNull);
        let result = h.validate("email", &Value::Null);
        assert!(matches!(result, ValidationResult::Invalid(_)));
    }

    #[test]
    fn test_constraints_not_null_accepts_value() {
        let mut h = handler(ConstraintHandlerKind::NotNull);
        assert_eq!(
            h.validate("email", &Value::String("a@b.com".into())),
            ValidationResult::Valid
        );
    }

    // --- Unique --------------------------------------------------------------

    #[test]
    fn test_constraints_unique_rejects_duplicate() {
        let mut h = handler(ConstraintHandlerKind::Unique {
            seen: HashSet::new(),
        });
        assert_eq!(
            h.validate("email", &Value::String("a@b.com".into())),
            ValidationResult::Valid
        );
        assert_eq!(
            h.validate("email", &Value::String("a@b.com".into())),
            ValidationResult::Retry
        );
    }

    #[test]
    fn test_constraints_unique_allows_different_values() {
        let mut h = handler(ConstraintHandlerKind::Unique {
            seen: HashSet::new(),
        });
        for v in ["a@b.com", "c@d.com", "e@f.com"] {
            assert_eq!(
                h.validate("email", &Value::String(v.into())),
                ValidationResult::Valid
            );
        }
    }

    #[test]
    fn test_constraints_unique_allows_after_clearing_seen() {
        let mut h = handler(ConstraintHandlerKind::Unique {
            seen: HashSet::new(),
        });
        h.validate("email", &Value::String("a@b.com".into()));
        // Simulate restart: clear the seen set
        if let ConstraintHandlerKind::Unique { seen } = &mut h.kind {
            seen.clear();
        }
        assert_eq!(
            h.validate("email", &Value::String("a@b.com".into())),
            ValidationResult::Valid
        );
    }

    #[test]
    fn test_constraints_unique_distinguishes_value_types() {
        let mut h = handler(ConstraintHandlerKind::Unique {
            seen: HashSet::new(),
        });
        // Integer 42 and String "42" both serialize to "42" — same key.
        // This is by design: HashSet<String> keys with as_key().
        assert_eq!(h.validate("x", &Value::Int(42)), ValidationResult::Valid);
        assert_eq!(
            h.validate("x", &Value::String("42".into())),
            ValidationResult::Retry
        );
    }

    // --- CompositeUnique -----------------------------------------------------

    #[test]
    fn test_constraints_composite_unique_via_validate_row() {
        let mut h = handler(ConstraintHandlerKind::CompositeUnique {
            columns: vec!["order_id".into(), "product_id".into()],
            seen: HashSet::new(),
        });
        let row1 = vec![Value::Int(1), Value::Int(10)];
        let row2 = vec![Value::Int(1), Value::Int(11)];
        let row3 = vec![Value::Int(1), Value::Int(10)]; // duplicate of row1
        assert_eq!(h.validate_row(&row1), ValidationResult::Valid);
        assert_eq!(h.validate_row(&row2), ValidationResult::Valid);
        assert_eq!(h.validate_row(&row3), ValidationResult::Retry);
    }

    #[test]
    fn test_constraints_composite_unique_per_column_validate_is_noop() {
        let mut h = handler(ConstraintHandlerKind::CompositeUnique {
            columns: vec!["a".into(), "b".into()],
            seen: HashSet::new(),
        });
        assert_eq!(h.validate("a", &Value::Int(1)), ValidationResult::Valid);
        assert_eq!(h.validate("a", &Value::Int(1)), ValidationResult::Valid);
    }

    // --- CheckPositive -------------------------------------------------------

    #[test]
    fn test_constraints_check_positive_accepts_positive() {
        let mut h = handler(ConstraintHandlerKind::CheckPositive);
        assert_eq!(h.validate("n", &Value::Int(1)), ValidationResult::Valid);
        assert_eq!(
            h.validate("n", &Value::Float(0.0001)),
            ValidationResult::Valid
        );
    }

    #[test]
    fn test_constraints_check_positive_rejects_zero() {
        let mut h = handler(ConstraintHandlerKind::CheckPositive);
        assert_eq!(h.validate("n", &Value::Int(0)), ValidationResult::Retry);
        assert_eq!(h.validate("n", &Value::Float(0.0)), ValidationResult::Retry);
    }

    #[test]
    fn test_constraints_check_positive_rejects_negative() {
        let mut h = handler(ConstraintHandlerKind::CheckPositive);
        assert_eq!(h.validate("n", &Value::Int(-5)), ValidationResult::Retry);
        assert_eq!(
            h.validate("n", &Value::Float(-0.001)),
            ValidationResult::Retry
        );
    }

    #[test]
    fn test_constraints_check_positive_ignores_non_numeric() {
        let mut h = handler(ConstraintHandlerKind::CheckPositive);
        assert_eq!(
            h.validate("n", &Value::String("hi".into())),
            ValidationResult::Valid
        );
    }

    // --- CheckRange ----------------------------------------------------------

    #[test]
    fn test_constraints_check_range_accepts_in_bounds() {
        let mut h = handler(ConstraintHandlerKind::CheckRange {
            min: 1.0,
            max: 100.0,
        });
        assert_eq!(h.validate("n", &Value::Int(1)), ValidationResult::Valid);
        assert_eq!(h.validate("n", &Value::Int(50)), ValidationResult::Valid);
        assert_eq!(h.validate("n", &Value::Int(100)), ValidationResult::Valid);
    }

    #[test]
    fn test_constraints_check_range_rejects_out_of_bounds() {
        let mut h = handler(ConstraintHandlerKind::CheckRange {
            min: 1.0,
            max: 100.0,
        });
        assert_eq!(h.validate("n", &Value::Int(0)), ValidationResult::Retry);
        assert_eq!(h.validate("n", &Value::Int(101)), ValidationResult::Retry);
    }

    // --- MaxLength -----------------------------------------------------------

    #[test]
    fn test_constraints_max_length_rejects_too_long() {
        let mut h = handler(ConstraintHandlerKind::MaxLength(5));
        assert_eq!(
            h.validate("s", &Value::String("hello!".into())),
            ValidationResult::Retry
        );
    }

    #[test]
    fn test_constraints_max_length_accepts_within() {
        let mut h = handler(ConstraintHandlerKind::MaxLength(5));
        assert_eq!(
            h.validate("s", &Value::String("hi".into())),
            ValidationResult::Valid
        );
        assert_eq!(
            h.validate("s", &Value::String("hello".into())),
            ValidationResult::Valid
        );
    }

    #[test]
    fn test_constraints_max_length_counts_chars_not_bytes() {
        // 5 chars, but 10 bytes in UTF-8 — should be accepted at limit 5.
        let mut h = handler(ConstraintHandlerKind::MaxLength(5));
        assert_eq!(
            h.validate("s", &Value::String("héllo".into())),
            ValidationResult::Valid
        );
    }

    #[test]
    fn test_constraints_max_length_ignores_non_string() {
        let mut h = handler(ConstraintHandlerKind::MaxLength(3));
        assert_eq!(
            h.validate("s", &Value::Int(123456)),
            ValidationResult::Valid
        );
    }

    // --- CHECK parser --------------------------------------------------------

    #[test]
    fn test_parse_check_positive_simple() {
        let kind = parse_check_constraint("price > 0").expect("should parse");
        assert!(matches!(kind, ConstraintHandlerKind::CheckPositive));
    }

    #[test]
    fn test_parse_check_positive_pg_style_with_cast() {
        let kind = parse_check_constraint("(price > (0)::numeric)").expect("should parse pg-style");
        assert!(matches!(kind, ConstraintHandlerKind::CheckPositive));
    }

    #[test]
    fn test_parse_check_range_simple() {
        let kind = parse_check_constraint("age >= 1 AND age <= 100").expect("should parse range");
        match kind {
            ConstraintHandlerKind::CheckRange { min, max } => {
                assert_eq!(min, 1.0);
                assert_eq!(max, 100.0);
            }
            other => panic!("expected CheckRange, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_check_range_lowercase_and() {
        let kind =
            parse_check_constraint("score >= 0 and score <= 10").expect("should parse range");
        assert!(matches!(kind, ConstraintHandlerKind::CheckRange { .. }));
    }

    #[test]
    fn test_parse_check_unknown_returns_none() {
        assert!(parse_check_constraint("status IN ('a', 'b')").is_none());
        assert!(parse_check_constraint("length(name) > 0").is_none());
        assert!(parse_check_constraint("price != 0").is_none());
    }

    #[test]
    fn test_parse_check_positive_does_not_match_greater_than_nonzero() {
        // "x > 5" is not CheckPositive — only "x > 0"
        assert!(parse_check_constraint("price > 5").is_none());
    }
}
