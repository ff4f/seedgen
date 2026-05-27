pub mod cycles;
pub mod topological;

use crate::introspection::SchemaGraph;

pub use cycles::{detect_cycles, BreakableEdge, CycleReport};
pub use topological::{topological_sort, ResolverError};

#[derive(Debug, Clone)]
pub struct InsertionPlan {
    pub ordered_tables: Vec<String>,
    pub deferred_updates: Vec<DeferredUpdate>,
}

#[derive(Debug, Clone)]
pub struct DeferredUpdate {
    pub table: String,
    pub column: String,
    pub references_table: String,
}

pub fn resolve(schema: &SchemaGraph) -> Result<InsertionPlan, ResolverError> {
    let tables: Vec<String> = schema.tables.iter().map(|t| t.name.clone()).collect();

    match topological_sort(&tables, &schema.foreign_keys) {
        Ok(ordered_tables) => Ok(InsertionPlan {
            ordered_tables,
            deferred_updates: Vec::new(),
        }),
        Err(ResolverError::CyclicDependency { .. }) => {
            let report = detect_cycles(&tables, &schema.foreign_keys);

            let filtered_fks: Vec<_> = schema
                .foreign_keys
                .iter()
                .filter(|fk| {
                    !report.breakable_edges.iter().any(|be| {
                        be.from_table == fk.from_table
                            && be.from_column == fk.from_column
                            && be.to_table == fk.to_table
                    })
                })
                .cloned()
                .collect();

            let ordered_tables = topological_sort(&tables, &filtered_fks)?;

            let deferred_updates = report
                .breakable_edges
                .into_iter()
                .map(|be| DeferredUpdate {
                    table: be.from_table,
                    column: be.from_column,
                    references_table: be.to_table,
                })
                .collect();

            Ok(InsertionPlan {
                ordered_tables,
                deferred_updates,
            })
        }
        Err(other) => Err(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::introspection::{Column, DataType, ForeignKey, SchemaGraph, Table};

    fn col(name: &str, nullable: bool) -> Column {
        Column {
            name: name.into(),
            data_type: DataType::Integer,
            is_nullable: nullable,
            is_identity: false,
            is_generated: false,
            default_value: None,
            max_length: None,
            numeric_precision: None,
            numeric_scale: None,
        }
    }

    fn table(name: &str, cols: Vec<Column>) -> Table {
        Table {
            name: name.into(),
            columns: cols,
            constraints: Vec::new(),
        }
    }

    fn fk(from: &str, from_col: &str, to: &str, nullable: bool, deferrable: bool) -> ForeignKey {
        ForeignKey {
            from_table: from.into(),
            from_column: from_col.into(),
            to_table: to.into(),
            to_column: "id".into(),
            is_nullable: nullable,
            is_deferrable: deferrable,
        }
    }

    #[test]
    fn test_resolve_acyclic_blog_schema() {
        let schema = SchemaGraph {
            tables: vec![
                table("users", vec![col("id", false)]),
                table("posts", vec![col("id", false), col("user_id", false)]),
                table(
                    "comments",
                    vec![
                        col("id", false),
                        col("post_id", false),
                        col("user_id", false),
                    ],
                ),
            ],
            foreign_keys: vec![
                fk("posts", "user_id", "users", false, false),
                fk("comments", "post_id", "posts", false, false),
                fk("comments", "user_id", "users", false, false),
            ],
            enums: vec![],
        };

        let plan = resolve(&schema).expect("acyclic schema should resolve");
        assert_eq!(plan.deferred_updates.len(), 0);
        assert_eq!(plan.ordered_tables, vec!["users", "posts", "comments"]);
    }

    #[test]
    fn test_resolve_breakable_cycle_via_nullable_fk() {
        // departments.head_employee_id (nullable) → employees.id
        // employees.department_id (NOT NULL) → departments.id
        let schema = SchemaGraph {
            tables: vec![
                table(
                    "departments",
                    vec![col("id", false), col("head_employee_id", true)],
                ),
                table(
                    "employees",
                    vec![col("id", false), col("department_id", false)],
                ),
            ],
            foreign_keys: vec![
                fk("departments", "head_employee_id", "employees", true, false),
                fk("employees", "department_id", "departments", false, false),
            ],
            enums: vec![],
        };

        let plan = resolve(&schema).expect("nullable cycle should be breakable");
        assert_eq!(plan.ordered_tables, vec!["departments", "employees"]);
        assert_eq!(plan.deferred_updates.len(), 1);
        let upd = &plan.deferred_updates[0];
        assert_eq!(upd.table, "departments");
        assert_eq!(upd.column, "head_employee_id");
        assert_eq!(upd.references_table, "employees");
    }

    #[test]
    fn test_resolve_unbreakable_cycle_errors() {
        let schema = SchemaGraph {
            tables: vec![
                table("a", vec![col("id", false), col("b_id", false)]),
                table("b", vec![col("id", false), col("a_id", false)]),
            ],
            foreign_keys: vec![
                fk("a", "b_id", "b", false, false),
                fk("b", "a_id", "a", false, false),
            ],
            enums: vec![],
        };

        let err = resolve(&schema).expect_err("unbreakable cycle should error");
        match err {
            ResolverError::CyclicDependency { tables } => {
                assert!(tables.contains(&"a".to_string()));
                assert!(tables.contains(&"b".to_string()));
            }
            other => panic!("expected CyclicDependency, got {other:?}"),
        }
    }
}
