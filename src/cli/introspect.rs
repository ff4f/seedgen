use anyhow::Context;
use sqlx::PgPool;

use crate::introspection::{introspect, SchemaGraph};

#[derive(Debug)]
pub struct Args {
    pub format: String,
    pub output: Option<String>,
    pub include: Option<Vec<String>>,
    pub exclude: Option<Vec<String>>,
}

pub async fn run(args: Args, url: &str) -> anyhow::Result<()> {
    let pool = PgPool::connect(url)
        .await
        .context("failed to connect to database")?;

    let mut schema = introspect(&pool).await?;
    apply_filters(&mut schema, &args.include, &args.exclude);

    let rendered = match args.format.as_str() {
        "json" => {
            serde_json::to_string_pretty(&schema).context("failed to serialize schema to JSON")?
        }
        "yaml" => serde_yaml::to_string(&schema).context("failed to serialize schema to YAML")?,
        "table" => render_table(&schema),
        other => anyhow::bail!("unknown format: `{other}` (expected table/json/yaml)"),
    };

    if let Some(path) = args.output {
        std::fs::write(&path, &rendered).with_context(|| format!("failed to write to `{path}`"))?;
    } else {
        print!("{rendered}");
        if !rendered.ends_with('\n') {
            println!();
        }
    }
    Ok(())
}

fn apply_filters(
    schema: &mut SchemaGraph,
    include: &Option<Vec<String>>,
    exclude: &Option<Vec<String>>,
) {
    if let Some(inc) = include {
        let set: std::collections::HashSet<&str> = inc.iter().map(|s| s.as_str()).collect();
        schema.tables.retain(|t| set.contains(t.name.as_str()));
        schema.foreign_keys.retain(|fk| {
            set.contains(fk.from_table.as_str()) && set.contains(fk.to_table.as_str())
        });
    }
    if let Some(exc) = exclude {
        let set: std::collections::HashSet<&str> = exc.iter().map(|s| s.as_str()).collect();
        schema.tables.retain(|t| !set.contains(t.name.as_str()));
        schema.foreign_keys.retain(|fk| {
            !set.contains(fk.from_table.as_str()) && !set.contains(fk.to_table.as_str())
        });
    }
}

fn render_table(schema: &SchemaGraph) -> String {
    let mut out = String::new();
    out.push_str(&format!("Tables ({}):\n", schema.tables.len()));
    for table in &schema.tables {
        let nullable_count = table.columns.iter().filter(|c| c.is_nullable).count();
        out.push_str(&format!(
            "  {:<30} {} columns ({} nullable)\n",
            table.name,
            table.columns.len(),
            nullable_count,
        ));
    }

    if !schema.foreign_keys.is_empty() {
        out.push_str(&format!(
            "\nForeign Keys ({}):\n",
            schema.foreign_keys.len()
        ));
        for fk in &schema.foreign_keys {
            let marker = if fk.is_nullable { " [nullable]" } else { "" };
            out.push_str(&format!(
                "  {}.{} -> {}.{}{}\n",
                fk.from_table, fk.from_column, fk.to_table, fk.to_column, marker,
            ));
        }
    }

    if !schema.enums.is_empty() {
        out.push_str(&format!("\nEnums ({}):\n", schema.enums.len()));
        for e in &schema.enums {
            out.push_str(&format!("  {}: [{}]\n", e.name, e.values.join(", ")));
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::introspection::{
        Column, ConstraintKind, DataType, EnumType, ForeignKey, SchemaGraph, Table,
    };

    fn sample_schema() -> SchemaGraph {
        SchemaGraph {
            tables: vec![
                Table {
                    name: "users".into(),
                    columns: vec![Column {
                        name: "id".into(),
                        data_type: DataType::Integer,
                        is_nullable: false,
                        is_identity: true,
                        is_generated: false,
                        default_value: None,
                        max_length: None,
                        numeric_precision: None,
                        numeric_scale: None,
                    }],
                    constraints: vec![],
                },
                Table {
                    name: "posts".into(),
                    columns: vec![],
                    constraints: vec![],
                },
            ],
            foreign_keys: vec![ForeignKey {
                from_table: "posts".into(),
                from_column: "user_id".into(),
                to_table: "users".into(),
                to_column: "id".into(),
                is_nullable: false,
                is_deferrable: false,
            }],
            enums: vec![EnumType {
                name: "status".into(),
                values: vec!["a".into(), "b".into()],
            }],
        }
    }

    #[test]
    fn test_render_table_includes_tables_fks_enums() {
        let out = render_table(&sample_schema());
        assert!(out.contains("Tables (2)"));
        assert!(out.contains("users"));
        assert!(out.contains("posts"));
        assert!(out.contains("Foreign Keys (1)"));
        assert!(out.contains("posts.user_id -> users.id"));
        assert!(out.contains("Enums (1)"));
        assert!(out.contains("status"));
    }

    #[test]
    fn test_apply_filters_include_keeps_only_listed() {
        let mut s = sample_schema();
        apply_filters(&mut s, &Some(vec!["users".into()]), &None);
        assert_eq!(s.tables.len(), 1);
        assert_eq!(s.tables[0].name, "users");
        assert_eq!(s.foreign_keys.len(), 0); // FK dropped (posts not in include list)
    }

    #[test]
    fn test_apply_filters_exclude_drops_listed() {
        let mut s = sample_schema();
        apply_filters(&mut s, &None, &Some(vec!["posts".into()]));
        assert_eq!(s.tables.len(), 1);
        assert_eq!(s.tables[0].name, "users");
        assert_eq!(s.foreign_keys.len(), 0);
    }

    // Suppress unused import warnings in this test module.
    #[allow(dead_code)]
    fn _silence(_: ConstraintKind) {}
}
