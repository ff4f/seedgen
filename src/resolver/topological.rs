use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};

use crate::introspection::ForeignKey;

#[derive(Debug, thiserror::Error)]
pub enum ResolverError {
    #[error("cyclic foreign-key dependency among tables: {tables:?}")]
    CyclicDependency { tables: Vec<String> },

    #[error("foreign key references unknown table: {0}")]
    TableNotFound(String),
}

pub fn topological_sort(
    tables: &[String],
    foreign_keys: &[ForeignKey],
) -> Result<Vec<String>, ResolverError> {
    let table_set: BTreeSet<&str> = tables.iter().map(String::as_str).collect();

    for fk in foreign_keys {
        if !table_set.contains(fk.from_table.as_str()) {
            return Err(ResolverError::TableNotFound(fk.from_table.clone()));
        }
        if !table_set.contains(fk.to_table.as_str()) {
            return Err(ResolverError::TableNotFound(fk.to_table.clone()));
        }
    }

    let mut adjacency: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    let mut in_degree: BTreeMap<&str, usize> = BTreeMap::new();
    for t in tables {
        adjacency.entry(t.as_str()).or_default();
        in_degree.entry(t.as_str()).or_insert(0);
    }

    for fk in foreign_keys {
        if fk.from_table == fk.to_table {
            continue;
        }
        let added = adjacency
            .get_mut(fk.to_table.as_str())
            .expect("table presence validated above")
            .insert(fk.from_table.as_str());
        if added {
            *in_degree
                .get_mut(fk.from_table.as_str())
                .expect("table presence validated above") += 1;
        }
    }

    let mut queue: BinaryHeap<Reverse<&str>> = in_degree
        .iter()
        .filter(|(_, &deg)| deg == 0)
        .map(|(&t, _)| Reverse(t))
        .collect();

    let mut order: Vec<String> = Vec::with_capacity(tables.len());
    while let Some(Reverse(name)) = queue.pop() {
        order.push(name.to_string());
        if let Some(children) = adjacency.get(name) {
            for &child in children {
                let deg = in_degree
                    .get_mut(child)
                    .expect("child present in in_degree");
                *deg -= 1;
                if *deg == 0 {
                    queue.push(Reverse(child));
                }
            }
        }
    }

    if order.len() != tables.len() {
        let remaining: Vec<String> = in_degree
            .iter()
            .filter(|(_, &deg)| deg > 0)
            .map(|(&t, _)| t.to_string())
            .collect();
        return Err(ResolverError::CyclicDependency { tables: remaining });
    }

    Ok(order)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::introspection::ForeignKey;

    fn fk(from: &str, to: &str) -> ForeignKey {
        ForeignKey {
            from_table: from.to_string(),
            from_column: "x".to_string(),
            to_table: to.to_string(),
            to_column: "id".to_string(),
            is_nullable: false,
            is_deferrable: false,
        }
    }

    fn tables(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    fn position(order: &[String], name: &str) -> usize {
        order
            .iter()
            .position(|t| t == name)
            .unwrap_or_else(|| panic!("`{name}` missing from order {order:?}"))
    }

    #[test]
    fn test_topological_sort_linear_chain() {
        // a → b → c (a references b, b references c). Insert leaves first.
        let order =
            topological_sort(&tables(&["a", "b", "c"]), &[fk("a", "b"), fk("b", "c")]).unwrap();
        assert_eq!(order, vec!["c", "b", "a"]);
    }

    #[test]
    fn test_topological_sort_diamond() {
        // a → b, a → c, b → d, c → d
        // d has no parents and must come first; a depends on b and c and must come last.
        let order = topological_sort(
            &tables(&["a", "b", "c", "d"]),
            &[fk("a", "b"), fk("a", "c"), fk("b", "d"), fk("c", "d")],
        )
        .unwrap();

        assert_eq!(order.len(), 4);
        assert_eq!(order[0], "d");
        assert_eq!(order[3], "a");
        assert!(position(&order, "b") < position(&order, "a"));
        assert!(position(&order, "c") < position(&order, "a"));
        assert!(position(&order, "d") < position(&order, "b"));
        assert!(position(&order, "d") < position(&order, "c"));
    }

    #[test]
    fn test_topological_sort_no_dependencies() {
        // Independent tables: returned in deterministic alphabetical order regardless of input.
        let order = topological_sort(&tables(&["zebra", "apple", "mango"]), &[]).unwrap();
        assert_eq!(order, vec!["apple", "mango", "zebra"]);
    }

    #[test]
    fn test_topological_sort_self_reference_is_not_a_cycle() {
        // categories.parent_id REFERENCES categories.id — single-table self-ref is allowed.
        let order = topological_sort(
            &tables(&["categories", "users"]),
            &[fk("categories", "categories")],
        )
        .unwrap();
        assert_eq!(order.len(), 2);
        assert!(order.contains(&"categories".to_string()));
        assert!(order.contains(&"users".to_string()));
    }

    #[test]
    fn test_topological_sort_two_node_cycle_is_detected() {
        let err = topological_sort(&tables(&["a", "b"]), &[fk("a", "b"), fk("b", "a")])
            .expect_err("two-node cycle should error");
        match err {
            ResolverError::CyclicDependency { tables } => {
                assert!(tables.contains(&"a".to_string()));
                assert!(tables.contains(&"b".to_string()));
            }
            other => panic!("expected CyclicDependency, got {other:?}"),
        }
    }

    #[test]
    fn test_topological_sort_unknown_table_in_fk_errors() {
        let err = topological_sort(&tables(&["a"]), &[fk("a", "ghost")])
            .expect_err("FK to unknown table should error");
        matches!(err, ResolverError::TableNotFound(t) if t == "ghost");
    }

    #[test]
    fn test_topological_sort_composite_fk_deduped() {
        // Composite FK from a to b emits two ForeignKey rows; should still produce one edge.
        let order = topological_sort(
            &tables(&["a", "b"]),
            &[
                ForeignKey {
                    from_table: "a".into(),
                    from_column: "b_id1".into(),
                    to_table: "b".into(),
                    to_column: "id1".into(),
                    is_nullable: false,
                    is_deferrable: false,
                },
                ForeignKey {
                    from_table: "a".into(),
                    from_column: "b_id2".into(),
                    to_table: "b".into(),
                    to_column: "id2".into(),
                    is_nullable: false,
                    is_deferrable: false,
                },
            ],
        )
        .unwrap();
        assert_eq!(order, vec!["b", "a"]);
    }
}
