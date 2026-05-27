use std::collections::HashMap;

use serde::Deserialize;
use serde_yaml::Value as Yaml;

#[derive(Debug, Clone, PartialEq)]
pub struct ScenarioConfig {
    pub seed: Option<u64>,
    pub tables: HashMap<String, TableScenario>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TableScenario {
    pub count: CountExpression,
    pub overrides: HashMap<String, ColumnOverride>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CountExpression {
    Fixed(usize),
    PerParent {
        parent_table: String,
        min: usize,
        max: usize,
    },
    PercentageOf {
        table: String,
        percentage: f64,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ColumnOverride {
    Distribution(HashMap<String, f64>),
    Range {
        min: f64,
        max: f64,
    },
    Formula(String),
    AfterParent {
        parent_table: String,
        parent_column: String,
    },
    FromParent {
        parent_table: String,
        parent_column: String,
    },
    Generator {
        name: String,
        params: HashMap<String, String>,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ScenarioError {
    #[error("YAML parse error: {0}")]
    Yaml(#[from] serde_yaml::Error),

    #[error("invalid count expression: {0}")]
    InvalidCount(String),

    #[error("invalid override for `{column}`: {reason}")]
    InvalidOverride { column: String, reason: String },
}

#[derive(Debug, Deserialize)]
struct RawScenario {
    #[serde(default)]
    seed: Option<u64>,
    #[serde(default)]
    tables: HashMap<String, RawTable>,
}

#[derive(Debug, Deserialize)]
struct RawTable {
    count: Yaml,
    #[serde(default)]
    overrides: HashMap<String, Yaml>,
}

pub fn parse_scenario(yaml_content: &str) -> Result<ScenarioConfig, ScenarioError> {
    let raw: RawScenario = serde_yaml::from_str(yaml_content)?;

    let mut tables = HashMap::with_capacity(raw.tables.len());
    for (name, raw_table) in raw.tables {
        let count = parse_count(&raw_table.count)?;
        let mut overrides = HashMap::with_capacity(raw_table.overrides.len());
        for (col, value) in raw_table.overrides {
            let ov = parse_override(&col, &value)?;
            overrides.insert(col, ov);
        }
        tables.insert(name, TableScenario { count, overrides });
    }

    Ok(ScenarioConfig {
        seed: raw.seed,
        tables,
    })
}

fn parse_count(v: &Yaml) -> Result<CountExpression, ScenarioError> {
    if let Some(n) = v.as_u64() {
        return Ok(CountExpression::Fixed(n as usize));
    }
    if let Some(s) = v.as_str() {
        if let Some(c) = parse_per_parent(s) {
            return Ok(c);
        }
        if let Some(c) = parse_percentage_of(s) {
            return Ok(c);
        }
        if let Ok(n) = s.parse::<usize>() {
            return Ok(CountExpression::Fixed(n));
        }
    }
    Err(ScenarioError::InvalidCount(format!("{v:?}")))
}

fn parse_per_parent(s: &str) -> Option<CountExpression> {
    let inner = s.strip_prefix("per_parent(")?.strip_suffix(')')?;
    let (parent, range) = inner.split_once(',')?;
    let (lo, hi) = range.trim().split_once("..")?;
    let min: usize = lo.trim().parse().ok()?;
    let max: usize = hi.trim().parse().ok()?;
    if min > max {
        return None;
    }
    Some(CountExpression::PerParent {
        parent_table: parent.trim().to_string(),
        min,
        max,
    })
}

fn parse_percentage_of(s: &str) -> Option<CountExpression> {
    let (lhs, rhs) = s.split_once(" of ")?;
    let pct_str = lhs.trim().trim_end_matches('%');
    let percentage: f64 = pct_str.parse().ok()?;
    Some(CountExpression::PercentageOf {
        table: rhs.trim().to_string(),
        percentage,
    })
}

fn parse_override(column: &str, v: &Yaml) -> Result<ColumnOverride, ScenarioError> {
    let bad = |reason: String| ScenarioError::InvalidOverride {
        column: column.to_string(),
        reason,
    };

    let map = v
        .as_mapping()
        .ok_or_else(|| bad("expected a mapping".into()))?;

    if let Some(dist) = map.get(Yaml::from("distribution")) {
        return parse_distribution(column, dist);
    }
    if let Some(range) = map.get(Yaml::from("range")) {
        return parse_range(column, range);
    }
    if let Some(f) = map.get(Yaml::from("formula")) {
        let s = f
            .as_str()
            .ok_or_else(|| bad("formula must be a string".into()))?;
        return Ok(ColumnOverride::Formula(s.to_string()));
    }
    if let Some(after) = map.get(Yaml::from("after")) {
        let s = after
            .as_str()
            .ok_or_else(|| bad("after must be a string `table.column`".into()))?;
        let (t, c) = s
            .split_once('.')
            .ok_or_else(|| bad("after must be `table.column`".into()))?;
        return Ok(ColumnOverride::AfterParent {
            parent_table: t.to_string(),
            parent_column: c.to_string(),
        });
    }
    if let Some(from) = map.get(Yaml::from("from_parent")) {
        let s = from
            .as_str()
            .ok_or_else(|| bad("from_parent must be a string `table.column`".into()))?;
        let (t, c) = s
            .split_once('.')
            .ok_or_else(|| bad("from_parent must be `table.column`".into()))?;
        return Ok(ColumnOverride::FromParent {
            parent_table: t.to_string(),
            parent_column: c.to_string(),
        });
    }
    if let Some(g) = map.get(Yaml::from("generator")) {
        return parse_generator(column, g);
    }

    let known = map
        .iter()
        .filter_map(|(k, _)| k.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    Err(bad(format!("unknown override keys: {known}")))
}

fn parse_distribution(column: &str, v: &Yaml) -> Result<ColumnOverride, ScenarioError> {
    let bad = |reason: String| ScenarioError::InvalidOverride {
        column: column.to_string(),
        reason,
    };
    let map = v
        .as_mapping()
        .ok_or_else(|| bad("distribution must be a mapping".into()))?;

    let mut dist = HashMap::with_capacity(map.len());
    for (k, val) in map {
        let key = k
            .as_str()
            .ok_or_else(|| bad("distribution keys must be strings".into()))?
            .to_string();
        let pct = parse_percentage_value(val).map_err(|e| bad(format!("`{key}`: {e}")))?;
        dist.insert(key, pct);
    }
    Ok(ColumnOverride::Distribution(dist))
}

fn parse_percentage_value(v: &Yaml) -> Result<f64, String> {
    if let Some(s) = v.as_str() {
        let cleaned = s.trim().trim_end_matches('%').trim();
        return cleaned
            .parse::<f64>()
            .map_err(|e| format!("invalid percentage `{s}`: {e}"));
    }
    if let Some(n) = v.as_f64() {
        return Ok(n);
    }
    if let Some(n) = v.as_u64() {
        return Ok(n as f64);
    }
    if let Some(n) = v.as_i64() {
        return Ok(n as f64);
    }
    Err(format!("invalid percentage: {v:?}"))
}

fn parse_range(column: &str, v: &Yaml) -> Result<ColumnOverride, ScenarioError> {
    let bad = |reason: String| ScenarioError::InvalidOverride {
        column: column.to_string(),
        reason,
    };
    let seq = v
        .as_sequence()
        .ok_or_else(|| bad("range must be a `[min, max]` sequence".into()))?;
    if seq.len() != 2 {
        return Err(bad(format!(
            "range must have exactly 2 elements, got {}",
            seq.len()
        )));
    }
    let min = value_to_f64(&seq[0]).map_err(|e| bad(format!("range min: {e}")))?;
    let max = value_to_f64(&seq[1]).map_err(|e| bad(format!("range max: {e}")))?;
    Ok(ColumnOverride::Range { min, max })
}

fn value_to_f64(v: &Yaml) -> Result<f64, String> {
    if let Some(n) = v.as_f64() {
        return Ok(n);
    }
    if let Some(n) = v.as_i64() {
        return Ok(n as f64);
    }
    if let Some(n) = v.as_u64() {
        return Ok(n as f64);
    }
    if let Some(s) = v.as_str() {
        // Try ISO date → days since 1970-01-01 (lets Range carry dates as numbers for v0.1).
        if let Ok(date) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
            let epoch = chrono::NaiveDate::from_ymd_opt(1970, 1, 1).expect("valid epoch");
            return Ok((date - epoch).num_days() as f64);
        }
        return s
            .parse::<f64>()
            .map_err(|e| format!("invalid number `{s}`: {e}"));
    }
    Err(format!("cannot convert to number: {v:?}"))
}

fn parse_generator(column: &str, v: &Yaml) -> Result<ColumnOverride, ScenarioError> {
    let bad = |reason: String| ScenarioError::InvalidOverride {
        column: column.to_string(),
        reason,
    };

    if let Some(s) = v.as_str() {
        return Ok(ColumnOverride::Generator {
            name: s.to_string(),
            params: HashMap::new(),
        });
    }
    if let Some(map) = v.as_mapping() {
        let name = map
            .get(Yaml::from("name"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| bad("generator.name is required".into()))?
            .to_string();
        let mut params = HashMap::new();
        if let Some(p) = map.get(Yaml::from("params")) {
            let pmap = p
                .as_mapping()
                .ok_or_else(|| bad("generator.params must be a mapping".into()))?;
            for (k, v) in pmap {
                let key = k
                    .as_str()
                    .ok_or_else(|| bad("generator.params keys must be strings".into()))?;
                let val = match v {
                    Yaml::String(s) => s.clone(),
                    Yaml::Number(n) => n.to_string(),
                    Yaml::Bool(b) => b.to_string(),
                    other => {
                        return Err(bad(format!(
                            "generator.params values must be scalar; got {other:?}"
                        )));
                    }
                };
                params.insert(key.to_string(), val);
            }
        }
        return Ok(ColumnOverride::Generator { name, params });
    }
    Err(bad(
        "generator must be a string or a {name, params} mapping".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = r#"
seed: 42
tables:
  users:
    count: 100
    overrides:
      role:
        distribution: { admin: 5%, moderator: 10%, user: 85% }
      created_at:
        range: [2023-01-01, 2024-12-31]
  orders:
    count: per_parent(users, 0..10)
    overrides:
      status:
        distribution: { pending: 10%, paid: 60%, shipped: 20%, delivered: 10% }
"#;

    fn assert_close(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-9, "{a} != {b}");
    }

    // --- The user's example end-to-end ----------------------------------------

    #[test]
    fn test_scenario_parse_full_example() {
        let cfg = parse_scenario(EXAMPLE).expect("parse failed");
        assert_eq!(cfg.seed, Some(42));
        assert_eq!(cfg.tables.len(), 2);

        let users = cfg.tables.get("users").expect("users missing");
        assert_eq!(users.count, CountExpression::Fixed(100));
        assert_eq!(users.overrides.len(), 2);

        let role = users.overrides.get("role").expect("role override missing");
        match role {
            ColumnOverride::Distribution(d) => {
                assert_close(d["admin"], 5.0);
                assert_close(d["moderator"], 10.0);
                assert_close(d["user"], 85.0);
            }
            other => panic!("expected Distribution, got {other:?}"),
        }

        let created = users
            .overrides
            .get("created_at")
            .expect("created_at missing");
        match created {
            ColumnOverride::Range { min, max } => {
                // 2023-01-01 = 19358 days since epoch; 2024-12-31 = 20088 days.
                assert_close(*min, 19358.0);
                assert_close(*max, 20088.0);
            }
            other => panic!("expected Range, got {other:?}"),
        }

        let orders = cfg.tables.get("orders").expect("orders missing");
        assert_eq!(
            orders.count,
            CountExpression::PerParent {
                parent_table: "users".into(),
                min: 0,
                max: 10,
            }
        );
        let status = orders.overrides.get("status").expect("status missing");
        match status {
            ColumnOverride::Distribution(d) => {
                assert_eq!(d.len(), 4);
                assert_close(
                    d["pending"] + d["paid"] + d["shipped"] + d["delivered"],
                    100.0,
                );
            }
            other => panic!("expected Distribution, got {other:?}"),
        }
    }

    // --- Count expressions ----------------------------------------------------

    #[test]
    fn test_scenario_parse_count_fixed_integer() {
        let cfg = parse_scenario("tables:\n  t:\n    count: 250\n").unwrap();
        assert_eq!(cfg.tables["t"].count, CountExpression::Fixed(250));
    }

    #[test]
    fn test_scenario_parse_count_per_parent() {
        let cfg =
            parse_scenario("tables:\n  orders:\n    count: per_parent(users, 1..5)\n").unwrap();
        assert_eq!(
            cfg.tables["orders"].count,
            CountExpression::PerParent {
                parent_table: "users".into(),
                min: 1,
                max: 5,
            }
        );
    }

    #[test]
    fn test_scenario_parse_count_percentage_of() {
        let cfg = parse_scenario("tables:\n  premium:\n    count: 20% of users\n").unwrap();
        match &cfg.tables["premium"].count {
            CountExpression::PercentageOf { table, percentage } => {
                assert_eq!(table, "users");
                assert_close(*percentage, 20.0);
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn test_scenario_parse_count_invalid_errors() {
        let err = parse_scenario("tables:\n  t:\n    count: nonsense\n").unwrap_err();
        assert!(matches!(err, ScenarioError::InvalidCount(_)));
    }

    #[test]
    fn test_scenario_parse_per_parent_inverted_range_errors() {
        let err =
            parse_scenario("tables:\n  t:\n    count: per_parent(users, 10..5)\n").unwrap_err();
        assert!(matches!(err, ScenarioError::InvalidCount(_)));
    }

    // --- Distribution overrides -----------------------------------------------

    #[test]
    fn test_scenario_parse_distribution_with_percent_strings() {
        let yaml = r#"
tables:
  t:
    count: 1
    overrides:
      role:
        distribution: { admin: 5%, user: 95% }
"#;
        let cfg = parse_scenario(yaml).unwrap();
        match &cfg.tables["t"].overrides["role"] {
            ColumnOverride::Distribution(d) => {
                assert_close(d["admin"], 5.0);
                assert_close(d["user"], 95.0);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn test_scenario_parse_distribution_with_plain_numbers() {
        let yaml = r#"
tables:
  t:
    count: 1
    overrides:
      role:
        distribution: { admin: 5, user: 95 }
"#;
        let cfg = parse_scenario(yaml).unwrap();
        match &cfg.tables["t"].overrides["role"] {
            ColumnOverride::Distribution(d) => {
                assert_close(d["admin"], 5.0);
                assert_close(d["user"], 95.0);
            }
            _ => panic!(),
        }
    }

    // --- Range overrides ------------------------------------------------------

    #[test]
    fn test_scenario_parse_range_numeric() {
        let yaml = r#"
tables:
  t:
    count: 1
    overrides:
      score:
        range: [0, 100]
"#;
        let cfg = parse_scenario(yaml).unwrap();
        match &cfg.tables["t"].overrides["score"] {
            ColumnOverride::Range { min, max } => {
                assert_close(*min, 0.0);
                assert_close(*max, 100.0);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn test_scenario_parse_range_dates() {
        let yaml = r#"
tables:
  t:
    count: 1
    overrides:
      d:
        range: [2023-01-01, 2023-01-02]
"#;
        let cfg = parse_scenario(yaml).unwrap();
        match &cfg.tables["t"].overrides["d"] {
            ColumnOverride::Range { min, max } => {
                // exactly 1 day apart
                assert_close(*max - *min, 1.0);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn test_scenario_parse_range_wrong_length_errors() {
        let yaml =
            "tables:\n  t:\n    count: 1\n    overrides:\n      x:\n        range: [1, 2, 3]\n";
        let err = parse_scenario(yaml).unwrap_err();
        match err {
            ScenarioError::InvalidOverride { column, .. } => assert_eq!(column, "x"),
            other => panic!("got {other:?}"),
        }
    }

    // --- Other override kinds -------------------------------------------------

    #[test]
    fn test_scenario_parse_formula() {
        let yaml = r#"
tables:
  t:
    count: 1
    overrides:
      total:
        formula: "qty * price"
"#;
        let cfg = parse_scenario(yaml).unwrap();
        match &cfg.tables["t"].overrides["total"] {
            ColumnOverride::Formula(expr) => assert_eq!(expr, "qty * price"),
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn test_scenario_parse_after_parent() {
        let yaml = r#"
tables:
  t:
    count: 1
    overrides:
      created_at:
        after: orders.placed_at
"#;
        let cfg = parse_scenario(yaml).unwrap();
        match &cfg.tables["t"].overrides["created_at"] {
            ColumnOverride::AfterParent {
                parent_table,
                parent_column,
            } => {
                assert_eq!(parent_table, "orders");
                assert_eq!(parent_column, "placed_at");
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn test_scenario_parse_from_parent() {
        let yaml = r#"
tables:
  t:
    count: 1
    overrides:
      currency:
        from_parent: orders.currency
"#;
        let cfg = parse_scenario(yaml).unwrap();
        match &cfg.tables["t"].overrides["currency"] {
            ColumnOverride::FromParent {
                parent_table,
                parent_column,
            } => {
                assert_eq!(parent_table, "orders");
                assert_eq!(parent_column, "currency");
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn test_scenario_parse_generator_short_form() {
        let yaml = r#"
tables:
  t:
    count: 1
    overrides:
      slug:
        generator: slug
"#;
        let cfg = parse_scenario(yaml).unwrap();
        match &cfg.tables["t"].overrides["slug"] {
            ColumnOverride::Generator { name, params } => {
                assert_eq!(name, "slug");
                assert!(params.is_empty());
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn test_scenario_parse_generator_with_params() {
        let yaml = r#"
tables:
  t:
    count: 1
    overrides:
      sku:
        generator:
          name: sku
          params:
            prefix: ACM
            length: 8
"#;
        let cfg = parse_scenario(yaml).unwrap();
        match &cfg.tables["t"].overrides["sku"] {
            ColumnOverride::Generator { name, params } => {
                assert_eq!(name, "sku");
                assert_eq!(params.get("prefix"), Some(&"ACM".to_string()));
                assert_eq!(params.get("length"), Some(&"8".to_string()));
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn test_scenario_parse_unknown_override_errors() {
        let yaml =
            "tables:\n  t:\n    count: 1\n    overrides:\n      x:\n        unknown_key: foo\n";
        let err = parse_scenario(yaml).unwrap_err();
        match err {
            ScenarioError::InvalidOverride { column, reason } => {
                assert_eq!(column, "x");
                assert!(reason.contains("unknown_key"));
            }
            other => panic!("got {other:?}"),
        }
    }

    // --- Top-level shape ------------------------------------------------------

    #[test]
    fn test_scenario_parse_missing_seed_defaults_to_none() {
        let cfg = parse_scenario("tables:\n  t:\n    count: 5\n").unwrap();
        assert_eq!(cfg.seed, None);
    }

    #[test]
    fn test_scenario_parse_empty_tables_is_ok() {
        let cfg = parse_scenario("seed: 1\ntables: {}\n").unwrap();
        assert_eq!(cfg.seed, Some(1));
        assert!(cfg.tables.is_empty());
    }

    #[test]
    fn test_scenario_parse_missing_count_errors() {
        let yaml = "tables:\n  t:\n    overrides: {}\n";
        let err = parse_scenario(yaml).unwrap_err();
        assert!(matches!(err, ScenarioError::Yaml(_)));
    }

    #[test]
    fn test_scenario_parse_invalid_yaml_errors() {
        let err = parse_scenario("tables:\n  t:\n  count: [bad").unwrap_err();
        assert!(matches!(err, ScenarioError::Yaml(_)));
    }
}
