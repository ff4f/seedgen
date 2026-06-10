use std::collections::{BTreeMap, HashMap};

use chrono::{Duration, NaiveDate};
use serde::Deserialize;
use serde_yaml::Value as Yaml;

use crate::generators::Value;
use crate::lifecycle::{
    BucketGranularity, ChurnModel, DurationRange, GrowthModel, LifecycleConfig, SeasonalityKind,
    SeasonalityModel, TableLifecycle, TemporalConstraint, TimelineDistribution,
};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ScenarioConfig {
    pub seed: Option<u64>,
    pub tables: HashMap<String, TableScenario>,
    /// Present only when the YAML declared a top-level `lifecycle:` block.
    pub lifecycle: Option<LifecycleConfig>,
    /// Per-table lifecycle behavior, for tables that declared a `growth:` block.
    pub table_lifecycles: HashMap<String, TableLifecycle>,
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

    #[error("invalid lifecycle config: {0}")]
    InvalidLifecycle(String),

    #[error("invalid growth for `{table}`: {reason}")]
    InvalidGrowth { table: String, reason: String },

    #[error("invalid churn for `{table}`: {reason}")]
    InvalidChurn { table: String, reason: String },

    #[error("invalid seasonality for `{table}`: {reason}")]
    InvalidSeasonality { table: String, reason: String },

    #[error("invalid temporal for `{table}.{column}`: {reason}")]
    InvalidTemporal {
        table: String,
        column: String,
        reason: String,
    },

    #[error("invalid timeline for `{table}.{column}`: {reason}")]
    InvalidTimeline {
        table: String,
        column: String,
        reason: String,
    },
}

#[derive(Debug, Deserialize)]
struct RawScenario {
    #[serde(default)]
    seed: Option<u64>,
    #[serde(default)]
    lifecycle: Option<RawLifecycle>,
    /// Tables are kept as raw YAML so lifecycle tables (which omit `count`) and
    /// normal tables (which require it) can take different parse paths.
    #[serde(default)]
    tables: HashMap<String, Yaml>,
}

#[derive(Debug, Deserialize)]
struct RawLifecycle {
    start: Yaml,
    end: Yaml,
    #[serde(default)]
    bucket: Option<String>,
}

/// Strict shape for a non-lifecycle table: `count` is required (a missing
/// `count` surfaces as a YAML error, preserving pre-lifecycle behavior).
#[derive(Debug, Deserialize)]
struct RawTable {
    count: Yaml,
    #[serde(default)]
    overrides: HashMap<String, Yaml>,
}

pub fn parse_scenario(yaml_content: &str) -> Result<ScenarioConfig, ScenarioError> {
    let raw: RawScenario = serde_yaml::from_str(yaml_content)?;

    let lifecycle = match &raw.lifecycle {
        Some(rl) => Some(parse_lifecycle(rl)?),
        None => None,
    };

    let mut tables = HashMap::with_capacity(raw.tables.len());
    let mut table_lifecycles = HashMap::new();

    for (name, table_yaml) in &raw.tables {
        if has_key(table_yaml, "growth") {
            // Lifecycle table: parse growth/churn/seasonality/temporal/timeline.
            let (ts, tlc) = parse_lifecycle_table(name, table_yaml)?;
            tables.insert(name.clone(), ts);
            table_lifecycles.insert(name.clone(), tlc);
        } else {
            // Normal table: strict deserialization (count required).
            let raw_table: RawTable = serde_yaml::from_value(table_yaml.clone())?;
            let count = parse_count(&raw_table.count)?;
            let mut overrides = HashMap::with_capacity(raw_table.overrides.len());
            for (col, value) in &raw_table.overrides {
                let ov = parse_override(col, value)?;
                overrides.insert(col.clone(), ov);
            }
            tables.insert(name.clone(), TableScenario { count, overrides });
        }
    }

    Ok(ScenarioConfig {
        seed: raw.seed,
        tables,
        lifecycle,
        table_lifecycles,
    })
}

/// Whether a YAML mapping contains `key`.
fn has_key(value: &Yaml, key: &str) -> bool {
    value
        .as_mapping()
        .and_then(|m| m.get(Yaml::from(key)))
        .is_some()
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
        let key =
            yaml_key_to_string(k).ok_or_else(|| bad("distribution keys must be scalars".into()))?;
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

// ===========================================================================
// Lifecycle parsing
// ===========================================================================

fn parse_lifecycle(raw: &RawLifecycle) -> Result<LifecycleConfig, ScenarioError> {
    let start = parse_date(&raw.start)
        .ok_or_else(|| ScenarioError::InvalidLifecycle("`start` must be YYYY-MM-DD".into()))?;
    let end = parse_date(&raw.end)
        .ok_or_else(|| ScenarioError::InvalidLifecycle("`end` must be YYYY-MM-DD".into()))?;
    if end <= start {
        return Err(ScenarioError::InvalidLifecycle(
            "`end` must be after `start`".into(),
        ));
    }
    let bucket = match raw.bucket.as_deref() {
        None | Some("month") => BucketGranularity::Month,
        Some("day") => BucketGranularity::Day,
        Some("week") => BucketGranularity::Week,
        Some("quarter") => BucketGranularity::Quarter,
        Some(other) => {
            return Err(ScenarioError::InvalidLifecycle(format!(
                "unknown bucket `{other}` (expected day/week/month/quarter)"
            )));
        }
    };
    Ok(LifecycleConfig { start, end, bucket })
}

/// Parse one lifecycle table into its (count/overrides) scenario plus its
/// lifecycle behavior. Assumes the mapping has a `growth:` block.
fn parse_lifecycle_table(
    table: &str,
    value: &Yaml,
) -> Result<(TableScenario, TableLifecycle), ScenarioError> {
    let map = value
        .as_mapping()
        .ok_or_else(|| ScenarioError::InvalidGrowth {
            table: table.to_string(),
            reason: "table must be a mapping".into(),
        })?;

    let growth_yaml =
        map.get(Yaml::from("growth"))
            .ok_or_else(|| ScenarioError::InvalidGrowth {
                table: table.to_string(),
                reason: "missing `growth` block".into(),
            })?;
    let growth = parse_growth(table, growth_yaml)?;

    let churn = match map.get(Yaml::from("churn")) {
        Some(y) => Some(parse_churn(table, y)?),
        None => None,
    };
    let seasonality = match map.get(Yaml::from("seasonality")) {
        Some(y) => Some(parse_seasonality(table, y)?),
        None => None,
    };

    let mut temporal_constraints = HashMap::new();
    if let Some(t) = map.get(Yaml::from("temporal")) {
        let tmap = t
            .as_mapping()
            .ok_or_else(|| ScenarioError::InvalidTemporal {
                table: table.to_string(),
                column: "*".into(),
                reason: "temporal must be a mapping".into(),
            })?;
        for (col, constraint) in tmap {
            let col = col.as_str().ok_or_else(|| ScenarioError::InvalidTemporal {
                table: table.to_string(),
                column: "*".into(),
                reason: "temporal keys must be column names".into(),
            })?;
            temporal_constraints.insert(col.to_string(), parse_temporal(table, col, constraint)?);
        }
    }

    let mut overrides = HashMap::new();
    let mut timeline_overrides = HashMap::new();
    if let Some(ov) = map.get(Yaml::from("overrides")) {
        let omap = ov
            .as_mapping()
            .ok_or_else(|| ScenarioError::InvalidOverride {
                column: "*".into(),
                reason: "overrides must be a mapping".into(),
            })?;
        for (col, col_value) in omap {
            let col = col.as_str().ok_or_else(|| ScenarioError::InvalidOverride {
                column: "*".into(),
                reason: "override keys must be column names".into(),
            })?;
            if let Some(tl) = col_value
                .as_mapping()
                .and_then(|m| m.get(Yaml::from("timeline")))
            {
                timeline_overrides.insert(col.to_string(), parse_timeline(table, col, tl)?);
            } else {
                overrides.insert(col.to_string(), parse_override(col, col_value)?);
            }
        }
    }

    // `count` is optional for lifecycle tables (growth drives per-bucket counts).
    let count = match map.get(Yaml::from("count")) {
        Some(c) => parse_count(c)?,
        None => CountExpression::Fixed(0),
    };

    Ok((
        TableScenario { count, overrides },
        TableLifecycle {
            growth,
            churn,
            seasonality,
            temporal_constraints,
            timeline_overrides,
        },
    ))
}

fn parse_growth(table: &str, value: &Yaml) -> Result<GrowthModel, ScenarioError> {
    let bad = |reason: String| ScenarioError::InvalidGrowth {
        table: table.to_string(),
        reason,
    };
    let map = value
        .as_mapping()
        .ok_or_else(|| bad("growth must be a mapping".into()))?;

    let f64_field = |key: &str| -> Result<Option<f64>, ScenarioError> {
        match map.get(Yaml::from(key)) {
            Some(y) => value_to_f64(y).map(Some).map_err(bad),
            None => Ok(None),
        }
    };
    let require_f64 = |key: &str| -> Result<f64, ScenarioError> {
        f64_field(key)?.ok_or_else(|| bad(format!("missing `{key}`")))
    };

    // `follows` form: count is proportional to a parent table.
    if let Some(follows) = map.get(Yaml::from("follows")) {
        let parent_table = follows
            .as_str()
            .ok_or_else(|| bad("`follows` must be a table name".into()))?
            .to_string();
        let ratio = f64_field("ratio")?;
        let per_parent = match map.get(Yaml::from("per_parent")) {
            Some(y) => Some(
                parse_usize_range(y)
                    .ok_or_else(|| bad("`per_parent` must be `min..max`".into()))?,
            ),
            None => None,
        };
        let variance = f64_field("variance")?.unwrap_or(0.0);
        return Ok(GrowthModel::Follows {
            parent_table,
            ratio,
            per_parent,
            variance,
        });
    }

    let model = map
        .get(Yaml::from("model"))
        .and_then(|y| y.as_str())
        .ok_or_else(|| bad("growth needs a `model` or `follows`".into()))?;

    match model {
        "linear" => Ok(GrowthModel::Linear {
            initial: require_f64("initial")?,
            rate: require_f64("rate")?,
        }),
        "exponential" => Ok(GrowthModel::Exponential {
            initial: require_f64("initial")?,
            rate: require_f64("rate")?,
        }),
        "s_curve" => Ok(GrowthModel::SCurve {
            initial: require_f64("initial")?,
            capacity: require_f64("capacity")?,
            rate: require_f64("rate")?,
        }),
        "logistic" => {
            let initial = require_f64("initial")?;
            let capacity = require_f64("capacity")?;
            let rate = require_f64("rate")?;
            // Derive the midpoint when omitted (same default as `s_curve`).
            let midpoint = match f64_field("midpoint")? {
                Some(m) => m,
                None if initial > 0.0 && rate != 0.0 => (capacity / initial).ln() / rate,
                None => 0.0,
            };
            Ok(GrowthModel::Logistic {
                initial,
                capacity,
                rate,
                midpoint,
            })
        }
        "custom" => {
            let seq = map
                .get(Yaml::from("values"))
                .and_then(|y| y.as_sequence())
                .ok_or_else(|| bad("`custom` needs a `values` array".into()))?;
            let values = seq
                .iter()
                .map(|y| y.as_u64().map(|n| n as usize))
                .collect::<Option<Vec<usize>>>()
                .ok_or_else(|| bad("`values` must be non-negative integers".into()))?;
            Ok(GrowthModel::Custom { values })
        }
        other => Err(bad(format!("unknown growth model `{other}`"))),
    }
}

fn parse_churn(table: &str, value: &Yaml) -> Result<ChurnModel, ScenarioError> {
    let bad = |reason: String| ScenarioError::InvalidChurn {
        table: table.to_string(),
        reason,
    };
    let map = value
        .as_mapping()
        .ok_or_else(|| bad("churn must be a mapping".into()))?;

    let rate = match map.get(Yaml::from("rate")) {
        Some(y) => value_to_f64(y).map_err(bad)?,
        None => return Err(bad("missing `rate`".into())),
    };
    let grace_period = map
        .get(Yaml::from("grace_period"))
        .and_then(|y| y.as_u64())
        .unwrap_or(1) as usize;
    let column = map
        .get(Yaml::from("column"))
        .and_then(|y| y.as_str())
        .ok_or_else(|| bad("missing `column`".into()))?
        .to_string();
    let churn_value = map
        .get(Yaml::from("value"))
        .map(yaml_to_value)
        .ok_or_else(|| bad("missing `value`".into()))?;
    let cascade = map
        .get(Yaml::from("cascade"))
        .and_then(|y| y.as_bool())
        .unwrap_or(true);

    Ok(ChurnModel {
        rate,
        grace_period,
        column,
        value: churn_value,
        cascade,
    })
}

fn parse_seasonality(table: &str, value: &Yaml) -> Result<SeasonalityModel, ScenarioError> {
    let bad = |reason: String| ScenarioError::InvalidSeasonality {
        table: table.to_string(),
        reason,
    };
    let map = value
        .as_mapping()
        .ok_or_else(|| bad("seasonality must be a mapping".into()))?;

    let (key, kind, expected) = if map.get(Yaml::from("monthly")).is_some() {
        ("monthly", SeasonalityKind::Monthly, 12)
    } else if map.get(Yaml::from("quarterly")).is_some() {
        ("quarterly", SeasonalityKind::Quarterly, 4)
    } else if map.get(Yaml::from("weekly")).is_some() {
        ("weekly", SeasonalityKind::Weekly, 7)
    } else {
        return Err(bad(
            "expected one of `monthly`, `quarterly`, `weekly`".into()
        ));
    };

    let seq = map
        .get(Yaml::from(key))
        .and_then(|y| y.as_sequence())
        .ok_or_else(|| bad(format!("`{key}` must be an array")))?;
    let multipliers = seq
        .iter()
        .map(value_to_f64)
        .collect::<Result<Vec<f64>, String>>()
        .map_err(bad)?;
    if multipliers.len() != expected {
        return Err(bad(format!(
            "`{key}` needs exactly {expected} values, got {}",
            multipliers.len()
        )));
    }

    Ok(SeasonalityModel { multipliers, kind })
}

fn parse_temporal(
    table: &str,
    column: &str,
    value: &Yaml,
) -> Result<TemporalConstraint, ScenarioError> {
    let bad = |reason: String| ScenarioError::InvalidTemporal {
        table: table.to_string(),
        column: column.to_string(),
        reason,
    };
    let map = value
        .as_mapping()
        .ok_or_else(|| bad("temporal constraint must be a mapping".into()))?;

    let offset = match map.get(Yaml::from("offset")) {
        Some(y) => Some(parse_duration_range(y).map_err(bad)?),
        None => None,
    };

    let parent = |key: &str| -> Result<Option<(String, String)>, ScenarioError> {
        match map.get(Yaml::from(key)) {
            Some(y) => {
                let s = y
                    .as_str()
                    .ok_or_else(|| bad(format!("`{key}` must be `table.column`")))?;
                let (t, c) = s
                    .split_once('.')
                    .ok_or_else(|| bad(format!("`{key}` must be `table.column`")))?;
                Ok(Some((t.to_string(), c.to_string())))
            }
            None => Ok(None),
        }
    };

    if let Some((t, c)) = parent("after")? {
        return Ok(TemporalConstraint::After {
            table: t,
            column: c,
            offset,
        });
    }
    if let Some((t, c)) = parent("before")? {
        return Ok(TemporalConstraint::Before {
            table: t,
            column: c,
            offset,
        });
    }
    if let Some((t, c)) = parent("equals")? {
        return Ok(TemporalConstraint::Equals {
            table: t,
            column: c,
        });
    }
    Err(bad("expected one of `after`, `before`, `equals`".into()))
}

fn parse_timeline(
    table: &str,
    column: &str,
    value: &Yaml,
) -> Result<TimelineDistribution, ScenarioError> {
    let bad = |reason: String| ScenarioError::InvalidTimeline {
        table: table.to_string(),
        column: column.to_string(),
        reason,
    };
    let map = value
        .as_mapping()
        .ok_or_else(|| bad("timeline must be a mapping of date → distribution".into()))?;

    let mut keyframes = BTreeMap::new();
    for (date_key, dist_yaml) in map {
        let date =
            parse_date(date_key).ok_or_else(|| bad("keyframe keys must be YYYY-MM-DD".into()))?;
        let dist_map = dist_yaml
            .as_mapping()
            .ok_or_else(|| bad("each keyframe must be a distribution mapping".into()))?;
        let mut dist = HashMap::with_capacity(dist_map.len());
        for (label, pct) in dist_map {
            let label = yaml_key_to_string(label)
                .ok_or_else(|| bad("distribution keys must be scalars".into()))?;
            let weight = parse_percentage_value(pct).map_err(|e| bad(format!("`{label}`: {e}")))?;
            dist.insert(label, weight);
        }
        keyframes.insert(date, dist);
    }
    Ok(TimelineDistribution { keyframes })
}

// --- small scalar helpers --------------------------------------------------

/// Parse a `YYYY-MM-DD` scalar (serde_yaml represents unquoted dates as strings).
fn parse_date(value: &Yaml) -> Option<NaiveDate> {
    let s = value.as_str()?;
    NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
}

/// Parse an inclusive `min..max` range (string form or a `[min, max]` sequence).
fn parse_usize_range(value: &Yaml) -> Option<(usize, usize)> {
    if let Some(s) = value.as_str() {
        let (lo, hi) = s.split_once("..")?;
        let min: usize = lo.trim().parse().ok()?;
        let max: usize = hi.trim().parse().ok()?;
        return if min <= max { Some((min, max)) } else { None };
    }
    if let Some(seq) = value.as_sequence() {
        if seq.len() == 2 {
            let min = seq[0].as_u64()? as usize;
            let max = seq[1].as_u64()? as usize;
            return if min <= max { Some((min, max)) } else { None };
        }
    }
    None
}

/// Parse a `min..max` duration offset like `1d..60d` or `1h..12h`.
fn parse_duration_range(value: &Yaml) -> Result<DurationRange, String> {
    let s = value
        .as_str()
        .ok_or_else(|| "offset must be a string like `1d..60d`".to_string())?;
    let (lo, hi) = s
        .split_once("..")
        .ok_or_else(|| format!("offset must be `min..max`, got `{s}`"))?;
    Ok(DurationRange {
        min: parse_duration(lo.trim())?,
        max: parse_duration(hi.trim())?,
    })
}

/// Parse a single duration token: a number plus a unit suffix `w|d|h|m|s`.
fn parse_duration(token: &str) -> Result<Duration, String> {
    if token.len() < 2 {
        return Err(format!("invalid duration `{token}`"));
    }
    let (num, unit) = token.split_at(token.len() - 1);
    let n: i64 = num
        .parse()
        .map_err(|_| format!("invalid duration number in `{token}`"))?;
    match unit {
        "w" => Ok(Duration::weeks(n)),
        "d" => Ok(Duration::days(n)),
        "h" => Ok(Duration::hours(n)),
        "m" => Ok(Duration::minutes(n)),
        "s" => Ok(Duration::seconds(n)),
        other => Err(format!(
            "unknown duration unit `{other}` (expected w/d/h/m/s)"
        )),
    }
}

/// Coerce a YAML mapping key (string, integer, or bool) into a string. Lets
/// distributions use numeric labels like `{ 5: 35%, 4: 30% }`.
fn yaml_key_to_string(key: &Yaml) -> Option<String> {
    if let Some(s) = key.as_str() {
        return Some(s.to_string());
    }
    if let Some(n) = key.as_i64() {
        return Some(n.to_string());
    }
    if let Some(n) = key.as_u64() {
        return Some(n.to_string());
    }
    if let Some(b) = key.as_bool() {
        return Some(b.to_string());
    }
    None
}

/// Convert a YAML scalar into a generator [`Value`] (used for churn values).
fn yaml_to_value(value: &Yaml) -> Value {
    match value {
        Yaml::Bool(b) => Value::Bool(*b),
        Yaml::String(s) => Value::String(s.clone()),
        Yaml::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Int(i)
            } else if let Some(u) = n.as_u64() {
                Value::Int(u as i64)
            } else {
                Value::Float(n.as_f64().unwrap_or(0.0))
            }
        }
        _ => Value::Null,
    }
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

    // --- Lifecycle parsing ----------------------------------------------------

    const LIFECYCLE_EXAMPLE: &str = r#"
seed: 42
lifecycle:
  start: 2023-01-01
  end: 2026-06-01
  bucket: month
tables:
  users:
    growth:
      model: s_curve
      initial: 10
      capacity: 5000
      rate: 0.15
    churn:
      rate: 0.03
      grace_period: 2
      column: is_active
      value: false
    overrides:
      role:
        distribution: { admin: 2%, moderator: 5%, user: 93% }
      plan:
        timeline:
          2023-01-01: { pro: 80%, free: 20% }
          2025-06-01: { pro: 25%, free: 55%, enterprise: 20% }
  orders:
    growth:
      follows: users
      ratio: 3.2
      variance: 0.35
    seasonality:
      monthly: [1.0, 0.7, 0.85, 1.0, 1.1, 0.8, 0.7, 0.85, 1.2, 1.4, 1.8, 2.5]
    temporal:
      created_at:
        after: users.created_at
        offset: 1d..60d
  order_items:
    growth:
      follows: orders
      per_parent: 1..5
    temporal:
      created_at:
        equals: orders.created_at
"#;

    #[test]
    fn test_scenario_parse_lifecycle_config() {
        let cfg = parse_scenario(LIFECYCLE_EXAMPLE).expect("parse failed");
        let lc = cfg.lifecycle.expect("lifecycle present");
        assert_eq!(lc.start, NaiveDate::from_ymd_opt(2023, 1, 1).unwrap());
        assert_eq!(lc.end, NaiveDate::from_ymd_opt(2026, 6, 1).unwrap());
        assert_eq!(lc.bucket, BucketGranularity::Month);
        assert_eq!(cfg.table_lifecycles.len(), 3);
    }

    #[test]
    fn test_scenario_parse_lifecycle_growth_variants() {
        let cfg = parse_scenario(LIFECYCLE_EXAMPLE).unwrap();

        match &cfg.table_lifecycles["users"].growth {
            GrowthModel::SCurve {
                initial,
                capacity,
                rate,
            } => {
                assert_close(*initial, 10.0);
                assert_close(*capacity, 5000.0);
                assert_close(*rate, 0.15);
            }
            other => panic!("expected SCurve, got {other:?}"),
        }
        match &cfg.table_lifecycles["orders"].growth {
            GrowthModel::Follows {
                parent_table,
                ratio,
                variance,
                ..
            } => {
                assert_eq!(parent_table, "users");
                assert_close(ratio.unwrap(), 3.2);
                assert_close(*variance, 0.35);
            }
            other => panic!("expected Follows, got {other:?}"),
        }
        match &cfg.table_lifecycles["order_items"].growth {
            GrowthModel::Follows { per_parent, .. } => {
                assert_eq!(*per_parent, Some((1, 5)));
            }
            other => panic!("expected Follows, got {other:?}"),
        }
    }

    #[test]
    fn test_scenario_parse_lifecycle_churn_seasonality_temporal_timeline() {
        let cfg = parse_scenario(LIFECYCLE_EXAMPLE).unwrap();

        let users = &cfg.table_lifecycles["users"];
        let churn = users.churn.as_ref().expect("churn present");
        assert_close(churn.rate, 0.03);
        assert_eq!(churn.grace_period, 2);
        assert_eq!(churn.column, "is_active");
        assert_eq!(churn.value, Value::Bool(false));
        assert!(churn.cascade); // defaults to true

        // Timeline routed out of overrides into the lifecycle's timeline_overrides;
        // the non-timeline override stays on the table scenario.
        assert!(users.timeline_overrides.contains_key("plan"));
        let users_scenario = &cfg.tables["users"];
        assert!(users_scenario.overrides.contains_key("role"));
        assert!(!users_scenario.overrides.contains_key("plan"));

        let orders = &cfg.table_lifecycles["orders"];
        let seasonality = orders.seasonality.as_ref().expect("seasonality present");
        assert_eq!(seasonality.multipliers.len(), 12);
        assert_close(seasonality.multipliers[11], 2.5);

        match orders.temporal_constraints.get("created_at") {
            Some(TemporalConstraint::After {
                table,
                column,
                offset,
            }) => {
                assert_eq!(table, "users");
                assert_eq!(column, "created_at");
                let offset = offset.as_ref().expect("offset present");
                assert_eq!(offset.min, Duration::days(1));
                assert_eq!(offset.max, Duration::days(60));
            }
            other => panic!("expected After, got {other:?}"),
        }
        match cfg.table_lifecycles["order_items"]
            .temporal_constraints
            .get("created_at")
        {
            Some(TemporalConstraint::Equals { table, column }) => {
                assert_eq!(table, "orders");
                assert_eq!(column, "created_at");
            }
            other => panic!("expected Equals, got {other:?}"),
        }
    }

    #[test]
    fn test_scenario_parse_lifecycle_defaults_bucket_to_month() {
        let yaml = "lifecycle:\n  start: 2024-01-01\n  end: 2024-12-01\ntables: {}\n";
        let cfg = parse_scenario(yaml).unwrap();
        assert_eq!(cfg.lifecycle.unwrap().bucket, BucketGranularity::Month);
    }

    #[test]
    fn test_scenario_parse_lifecycle_rejects_end_before_start() {
        let yaml = "lifecycle:\n  start: 2024-12-01\n  end: 2024-01-01\ntables: {}\n";
        let err = parse_scenario(yaml).unwrap_err();
        assert!(matches!(err, ScenarioError::InvalidLifecycle(_)));
    }

    #[test]
    fn test_scenario_parse_lifecycle_rejects_unknown_bucket() {
        let yaml =
            "lifecycle:\n  start: 2024-01-01\n  end: 2024-12-01\n  bucket: fortnight\ntables: {}\n";
        let err = parse_scenario(yaml).unwrap_err();
        assert!(matches!(err, ScenarioError::InvalidLifecycle(_)));
    }

    #[test]
    fn test_scenario_parse_growth_linear_and_custom() {
        let yaml = r#"
lifecycle:
  start: 2024-01-01
  end: 2024-06-01
tables:
  a:
    growth: { model: linear, initial: 10, rate: 5 }
  b:
    growth: { model: custom, values: [3, 7, 11] }
"#;
        let cfg = parse_scenario(yaml).unwrap();
        assert_eq!(
            cfg.table_lifecycles["a"].growth,
            GrowthModel::Linear {
                initial: 10.0,
                rate: 5.0
            }
        );
        assert_eq!(
            cfg.table_lifecycles["b"].growth,
            GrowthModel::Custom {
                values: vec![3, 7, 11]
            }
        );
    }

    #[test]
    fn test_scenario_parse_growth_missing_field_errors() {
        let yaml = "lifecycle:\n  start: 2024-01-01\n  end: 2024-06-01\ntables:\n  a:\n    growth: { model: linear, initial: 10 }\n";
        let err = parse_scenario(yaml).unwrap_err();
        assert!(matches!(err, ScenarioError::InvalidGrowth { .. }));
    }

    // --- Backward compatibility (no lifecycle block) --------------------------

    #[test]
    fn test_scenario_parse_without_lifecycle_is_unchanged() {
        let cfg = parse_scenario(EXAMPLE).expect("parse failed");
        assert!(cfg.lifecycle.is_none());
        assert!(cfg.table_lifecycles.is_empty());
        assert_eq!(cfg.tables.len(), 2);
        assert_eq!(cfg.tables["users"].count, CountExpression::Fixed(100));
    }

    #[test]
    fn test_scenario_normal_table_still_requires_count() {
        // No lifecycle, no growth → count is mandatory (unchanged behavior).
        let err = parse_scenario("tables:\n  t:\n    overrides: {}\n").unwrap_err();
        assert!(matches!(err, ScenarioError::Yaml(_)));
    }
}
