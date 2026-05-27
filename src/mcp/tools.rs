use std::collections::HashMap;

use serde_json::{json, Value};
use sqlx::PgPool;

use crate::introspection::introspect;
use crate::mcp::server::{INVALID_PARAMS, SERVER_ERROR};
use crate::output::truncate_tables;
use crate::resolver::resolve;
use crate::scenario::{load_template, CountExpression, ScenarioConfig, TableScenario};
use crate::{generate as run_generate, GenerateConfig, OutputMode};

pub fn tool_list() -> Value {
    json!({
        "tools": [
            {
                "name": "seedgen_introspect",
                "description": "Introspect the database schema. Returns tables, columns, foreign keys, constraints, and detected semantic types. Use this to understand the database structure before generating seed data.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "connection_url": {
                            "type": "string",
                            "description": "PostgreSQL connection URL. If omitted, uses DATABASE_URL env var."
                        },
                        "format": {
                            "type": "string",
                            "enum": ["summary", "full", "json"],
                            "default": "summary",
                            "description": "Output format."
                        }
                    },
                    "required": []
                }
            },
            {
                "name": "seedgen_generate",
                "description": "Generate seed data based on the database schema. Automatically detects relationships, resolves insertion order, and generates realistic data. Supports built-in scenarios (ecommerce, saas, blog, social) or custom entity counts.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "connection_url": { "type": "string" },
                        "scenario": {
                            "type": "string",
                            "description": "Built-in scenario template name."
                        },
                        "entities": {
                            "type": "object",
                            "description": "Override row counts per table, e.g. {\"users\": 100, \"orders\": 500}.",
                            "additionalProperties": { "type": "integer" }
                        },
                        "seed": {
                            "type": "integer",
                            "description": "Seed for deterministic output. Same seed = same data."
                        },
                        "rows": {
                            "type": "integer",
                            "default": 10,
                            "description": "Default rows per table when no scenario is provided."
                        },
                        "truncate_first": {
                            "type": "boolean",
                            "default": false,
                            "description": "TRUNCATE target tables before inserting."
                        }
                    },
                    "required": []
                }
            },
            {
                "name": "seedgen_reset",
                "description": "Truncate all tables in safe order. Destructive — requires explicit confirmation. Refuses to operate on databases whose URL contains 'prod'.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "connection_url": { "type": "string" },
                        "confirm": {
                            "type": "boolean",
                            "description": "Must be true to proceed."
                        },
                        "tables": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Optional: only reset these tables. If omitted, resets ALL tables."
                        }
                    },
                    "required": ["confirm"]
                }
            },
            {
                "name": "seedgen_list_scenarios",
                "description": "List all available built-in scenario templates.",
                "inputSchema": {
                    "type": "object",
                    "properties": {},
                    "required": []
                }
            },
            {
                "name": "seedgen_validate",
                "description": "Validate a scenario configuration against the database schema. Reports errors and warnings. (Not yet implemented.)",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "connection_url": { "type": "string" },
                        "scenario_config": { "type": "object" }
                    },
                    "required": ["scenario_config"]
                }
            }
        ]
    })
}

pub async fn dispatch_call(params: Value) -> Result<Value, (i32, String)> {
    let name = params
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or((INVALID_PARAMS, "missing `name`".into()))?;
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let text = match name {
        "seedgen_introspect" => tool_introspect(&args).await?,
        "seedgen_generate" => tool_generate(&args).await?,
        "seedgen_reset" => tool_reset(&args).await?,
        "seedgen_list_scenarios" => tool_list_scenarios(&args)?,
        "seedgen_validate" => {
            return Err((SERVER_ERROR, "validate not yet implemented".into()));
        }
        other => {
            return Err((INVALID_PARAMS, format!("unknown tool: `{other}`")));
        }
    };

    Ok(json!({
        "content": [
            { "type": "text", "text": text }
        ]
    }))
}

// --- tool handlers ----------------------------------------------------------

async fn tool_introspect(args: &Value) -> Result<String, (i32, String)> {
    let url = resolve_url(args)?;
    let pool = connect(&url).await?;
    let schema = introspect(&pool)
        .await
        .map_err(|e| (SERVER_ERROR, format!("introspection failed: {e}")))?;
    let plan = resolve(&schema).map_err(|e| (SERVER_ERROR, format!("resolver failed: {e}")))?;

    let format = args
        .get("format")
        .and_then(|v| v.as_str())
        .unwrap_or("summary");

    match format {
        "json" => serde_json::to_string_pretty(&schema)
            .map_err(|e| (SERVER_ERROR, format!("serialize: {e}"))),
        "full" | "summary" => Ok(render_summary(&schema, &plan)),
        other => Err((INVALID_PARAMS, format!("unknown format `{other}`"))),
    }
}

async fn tool_generate(args: &Value) -> Result<String, (i32, String)> {
    let url = resolve_url(args)?;
    let pool = connect(&url).await?;

    let scenario_name = args.get("scenario").and_then(|v| v.as_str());
    let mut scenario = if let Some(name) = scenario_name {
        Some(load_template(name).map_err(|e| (INVALID_PARAMS, e.to_string()))?)
    } else {
        None
    };

    if let Some(entities) = args.get("entities").and_then(|v| v.as_object()) {
        let map: HashMap<String, usize> = entities
            .iter()
            .filter_map(|(k, v)| v.as_u64().map(|n| (k.clone(), n as usize)))
            .collect();
        scenario = Some(merge_entities(scenario, map));
    }

    let seed = args
        .get("seed")
        .and_then(|v| v.as_u64())
        .or_else(|| scenario.as_ref().and_then(|s| s.seed))
        .unwrap_or(42);

    let rows_per_table = args
        .get("rows")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(10);

    let truncate_first = args
        .get("truncate_first")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let config = GenerateConfig {
        seed,
        rows_per_table,
        scenario,
        output_mode: OutputMode::DirectInsert,
        include_tables: None,
        exclude_tables: None,
        truncate_first,
    };

    let result = run_generate(&pool, &config)
        .await
        .map_err(|e| (SERVER_ERROR, format!("generate failed: {e}")))?;

    let mut out = format!(
        "✓ Generated {} rows across {} tables in {:?} (seed: {})\n",
        result.total_rows,
        result.tables_seeded.len(),
        result.duration,
        result.seed_used,
    );
    for t in &result.tables_seeded {
        out.push_str(&format!("  {:<30} {:>6} rows\n", t.name, t.rows_inserted));
    }
    Ok(out)
}

async fn tool_reset(args: &Value) -> Result<String, (i32, String)> {
    let confirm = args
        .get("confirm")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !confirm {
        return Err((
            INVALID_PARAMS,
            "destructive operation — pass `confirm: true` to proceed".into(),
        ));
    }

    let url = resolve_url(args)?;
    if url.to_ascii_lowercase().contains("prod") {
        return Err((
            INVALID_PARAMS,
            "refusing to reset a database whose URL contains `prod`".into(),
        ));
    }

    let pool = connect(&url).await?;

    let tables: Vec<String> = match args.get("tables").and_then(|v| v.as_array()) {
        Some(arr) if !arr.is_empty() => arr
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect(),
        _ => {
            let schema = introspect(&pool)
                .await
                .map_err(|e| (SERVER_ERROR, format!("introspection failed: {e}")))?;
            let plan =
                resolve(&schema).map_err(|e| (SERVER_ERROR, format!("resolver failed: {e}")))?;
            let mut t = plan.ordered_tables;
            t.reverse();
            t
        }
    };

    if tables.is_empty() {
        return Ok("No tables to reset.".into());
    }

    truncate_tables(&pool, &tables)
        .await
        .map_err(|e| (SERVER_ERROR, format!("truncate failed: {e}")))?;
    Ok(format!(
        "Truncated {} table{}",
        tables.len(),
        if tables.len() == 1 { "" } else { "s" }
    ))
}

fn tool_list_scenarios(_args: &Value) -> Result<String, (i32, String)> {
    let names = crate::scenario::list_templates();
    let mut out = String::from("Available scenarios:\n");
    for name in names {
        out.push_str(&format!("  - {name}\n"));
    }
    Ok(out)
}

// --- helpers ----------------------------------------------------------------

fn resolve_url(args: &Value) -> Result<String, (i32, String)> {
    args.get("connection_url")
        .and_then(|v| v.as_str())
        .map(String::from)
        .or_else(|| std::env::var("DATABASE_URL").ok())
        .ok_or((
            INVALID_PARAMS,
            "no connection URL (pass `connection_url` or set DATABASE_URL)".into(),
        ))
}

async fn connect(url: &str) -> Result<PgPool, (i32, String)> {
    PgPool::connect(url)
        .await
        .map_err(|e| (SERVER_ERROR, format!("connection failed: {e}")))
}

fn merge_entities(
    base: Option<ScenarioConfig>,
    entities: HashMap<String, usize>,
) -> ScenarioConfig {
    let mut cfg = base.unwrap_or(ScenarioConfig {
        seed: None,
        tables: HashMap::new(),
    });
    for (name, count) in entities {
        cfg.tables
            .entry(name)
            .and_modify(|ts| ts.count = CountExpression::Fixed(count))
            .or_insert(TableScenario {
                count: CountExpression::Fixed(count),
                overrides: HashMap::new(),
            });
    }
    cfg
}

fn render_summary(
    schema: &crate::introspection::SchemaGraph,
    plan: &crate::resolver::InsertionPlan,
) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Tables: {}\nForeign Keys: {}\nEnums: {}\n\n",
        schema.tables.len(),
        schema.foreign_keys.len(),
        schema.enums.len(),
    ));
    out.push_str("Insertion Order:\n");
    for (i, t) in plan.ordered_tables.iter().enumerate() {
        out.push_str(&format!("  {}. {t}\n", i + 1));
    }
    if !plan.deferred_updates.is_empty() {
        out.push_str("\nDeferred FK updates (cycle resolution):\n");
        for d in &plan.deferred_updates {
            out.push_str(&format!(
                "  {}.{} -> {}\n",
                d.table, d.column, d.references_table
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_list_includes_all_five() {
        let v = tool_list();
        let tools = v["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 5);
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        for expected in [
            "seedgen_introspect",
            "seedgen_generate",
            "seedgen_reset",
            "seedgen_list_scenarios",
            "seedgen_validate",
        ] {
            assert!(names.contains(&expected), "missing `{expected}`");
        }
    }

    #[test]
    fn test_tool_list_each_tool_has_description_and_input_schema() {
        let v = tool_list();
        for t in v["tools"].as_array().unwrap() {
            assert!(t["name"].is_string());
            assert!(t["description"].is_string());
            assert!(
                t["inputSchema"]["type"].as_str() == Some("object"),
                "tool {:?} missing inputSchema.type=object",
                t["name"]
            );
        }
    }

    #[tokio::test]
    async fn test_list_scenarios_returns_all_templates() {
        let text = tool_list_scenarios(&json!({})).unwrap();
        for name in ["ecommerce", "saas", "blog", "social"] {
            assert!(text.contains(name), "missing `{name}`");
        }
    }

    #[tokio::test]
    async fn test_reset_without_confirm_errors() {
        let err = tool_reset(&json!({})).await.unwrap_err();
        assert_eq!(err.0, INVALID_PARAMS);
        assert!(err.1.contains("confirm"));
    }

    #[tokio::test]
    async fn test_reset_refuses_prod_url() {
        let err = tool_reset(&json!({
            "confirm": true,
            "connection_url": "postgres://localhost/prod_db"
        }))
        .await
        .unwrap_err();
        assert!(err.1.contains("prod"));
    }

    #[tokio::test]
    async fn test_resolve_url_falls_back_to_env() {
        std::env::set_var("DATABASE_URL", "postgres://test/test");
        let url = resolve_url(&json!({})).unwrap();
        assert_eq!(url, "postgres://test/test");
        std::env::remove_var("DATABASE_URL");
    }

    #[tokio::test]
    async fn test_resolve_url_missing_errors() {
        std::env::remove_var("DATABASE_URL");
        let err = resolve_url(&json!({})).unwrap_err();
        assert_eq!(err.0, INVALID_PARAMS);
    }

    #[test]
    fn test_merge_entities_into_empty() {
        let mut e = HashMap::new();
        e.insert("users".to_string(), 50);
        let cfg = merge_entities(None, e);
        assert_eq!(cfg.tables.len(), 1);
        assert_eq!(cfg.tables["users"].count, CountExpression::Fixed(50));
    }

    #[test]
    fn test_merge_entities_overrides_existing_count() {
        let base = load_template("ecommerce").unwrap();
        let mut e = HashMap::new();
        e.insert("users".to_string(), 999);
        let merged = merge_entities(Some(base), e);
        assert_eq!(merged.tables["users"].count, CountExpression::Fixed(999));
        // Other tables from the ecommerce template are preserved.
        assert!(merged.tables.contains_key("orders"));
    }
}
