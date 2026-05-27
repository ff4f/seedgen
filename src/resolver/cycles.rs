use std::collections::{BTreeMap, BTreeSet};

use crate::introspection::ForeignKey;

#[derive(Debug, Clone)]
pub struct CycleReport {
    pub has_cycles: bool,
    pub cycles: Vec<Vec<String>>,
    pub breakable_edges: Vec<BreakableEdge>,
}

#[derive(Debug, Clone)]
pub struct BreakableEdge {
    pub from_table: String,
    pub from_column: String,
    pub to_table: String,
    pub is_nullable: bool,
    pub is_deferrable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Color {
    White,
    Gray,
    Black,
}

pub fn detect_cycles(tables: &[String], foreign_keys: &[ForeignKey]) -> CycleReport {
    let mut adjacency: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    let mut color: BTreeMap<&str, Color> = BTreeMap::new();
    for t in tables {
        adjacency.insert(t.as_str(), BTreeSet::new());
        color.insert(t.as_str(), Color::White);
    }
    for fk in foreign_keys {
        if fk.from_table == fk.to_table {
            continue;
        }
        if let Some(set) = adjacency.get_mut(fk.from_table.as_str()) {
            set.insert(fk.to_table.as_str());
        }
    }

    let mut raw_cycles: Vec<Vec<String>> = Vec::new();
    let mut stack: Vec<&str> = Vec::new();
    for t in tables {
        if color[t.as_str()] == Color::White {
            dfs(
                t.as_str(),
                &adjacency,
                &mut color,
                &mut stack,
                &mut raw_cycles,
            );
        }
    }

    let mut unique: BTreeSet<Vec<String>> = BTreeSet::new();
    for cycle in raw_cycles {
        unique.insert(normalize(cycle));
    }
    let cycles: Vec<Vec<String>> = unique.into_iter().collect();

    let mut seen: BTreeSet<(String, String, String)> = BTreeSet::new();
    let mut breakable_edges: Vec<BreakableEdge> = Vec::new();
    for cycle in &cycles {
        for i in 0..cycle.len() {
            let from = &cycle[i];
            let to = &cycle[(i + 1) % cycle.len()];
            for fk in foreign_keys {
                if &fk.from_table == from
                    && &fk.to_table == to
                    && (fk.is_nullable || fk.is_deferrable)
                {
                    let key = (
                        fk.from_table.clone(),
                        fk.from_column.clone(),
                        fk.to_table.clone(),
                    );
                    if seen.insert(key) {
                        breakable_edges.push(BreakableEdge {
                            from_table: fk.from_table.clone(),
                            from_column: fk.from_column.clone(),
                            to_table: fk.to_table.clone(),
                            is_nullable: fk.is_nullable,
                            is_deferrable: fk.is_deferrable,
                        });
                    }
                }
            }
        }
    }
    breakable_edges.sort_by(|a, b| {
        (&a.from_table, &a.from_column, &a.to_table).cmp(&(
            &b.from_table,
            &b.from_column,
            &b.to_table,
        ))
    });

    CycleReport {
        has_cycles: !cycles.is_empty(),
        cycles,
        breakable_edges,
    }
}

fn dfs<'a>(
    node: &'a str,
    adjacency: &'a BTreeMap<&'a str, BTreeSet<&'a str>>,
    color: &mut BTreeMap<&'a str, Color>,
    stack: &mut Vec<&'a str>,
    cycles: &mut Vec<Vec<String>>,
) {
    color.insert(node, Color::Gray);
    stack.push(node);

    if let Some(neighbors) = adjacency.get(node) {
        for &neighbor in neighbors {
            match color[neighbor] {
                Color::White => dfs(neighbor, adjacency, color, stack, cycles),
                Color::Gray => {
                    let start = stack
                        .iter()
                        .position(|&n| n == neighbor)
                        .expect("gray neighbor must be on current path");
                    let cycle: Vec<String> = stack[start..].iter().map(|s| s.to_string()).collect();
                    cycles.push(cycle);
                }
                Color::Black => {}
            }
        }
    }

    color.insert(node, Color::Black);
    stack.pop();
}

fn normalize(cycle: Vec<String>) -> Vec<String> {
    if cycle.is_empty() {
        return cycle;
    }
    let min_idx = cycle
        .iter()
        .enumerate()
        .min_by(|a, b| a.1.cmp(b.1))
        .map(|(i, _)| i)
        .unwrap_or(0);
    let mut rotated = Vec::with_capacity(cycle.len());
    rotated.extend_from_slice(&cycle[min_idx..]);
    rotated.extend_from_slice(&cycle[..min_idx]);
    rotated
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fk(from: &str, col: &str, to: &str, nullable: bool, deferrable: bool) -> ForeignKey {
        ForeignKey {
            from_table: from.into(),
            from_column: col.into(),
            to_table: to.into(),
            to_column: "id".into(),
            is_nullable: nullable,
            is_deferrable: deferrable,
        }
    }

    fn tables(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_cycles_simple_cycle() {
        let report = detect_cycles(
            &tables(&["a", "b"]),
            &[
                fk("a", "b_id", "b", false, false),
                fk("b", "a_id", "a", false, false),
            ],
        );
        assert!(report.has_cycles);
        assert_eq!(report.cycles, vec![vec!["a".to_string(), "b".to_string()]]);
    }

    #[test]
    fn test_cycles_no_cycle() {
        let report = detect_cycles(
            &tables(&["users", "posts", "comments"]),
            &[
                fk("posts", "user_id", "users", false, false),
                fk("comments", "post_id", "posts", false, false),
                fk("comments", "user_id", "users", false, false),
            ],
        );
        assert!(!report.has_cycles);
        assert!(report.cycles.is_empty());
        assert!(report.breakable_edges.is_empty());
    }

    #[test]
    fn test_cycles_with_nullable_edge_is_breakable() {
        // a.b_id (nullable) → b.id, b.a_id (NOT NULL) → a.id
        let report = detect_cycles(
            &tables(&["a", "b"]),
            &[
                fk("a", "b_id", "b", true, false),
                fk("b", "a_id", "a", false, false),
            ],
        );
        assert!(report.has_cycles);
        assert_eq!(report.breakable_edges.len(), 1);
        let edge = &report.breakable_edges[0];
        assert_eq!(edge.from_table, "a");
        assert_eq!(edge.from_column, "b_id");
        assert_eq!(edge.to_table, "b");
        assert!(edge.is_nullable);
    }

    #[test]
    fn test_cycles_with_deferrable_edge_is_breakable() {
        let report = detect_cycles(
            &tables(&["a", "b"]),
            &[
                fk("a", "b_id", "b", false, true),
                fk("b", "a_id", "a", false, false),
            ],
        );
        assert!(report.has_cycles);
        assert_eq!(report.breakable_edges.len(), 1);
        assert_eq!(report.breakable_edges[0].from_table, "a");
        assert!(report.breakable_edges[0].is_deferrable);
    }

    #[test]
    fn test_cycles_without_nullable_or_deferrable_edge_is_unbreakable() {
        let report = detect_cycles(
            &tables(&["a", "b"]),
            &[
                fk("a", "b_id", "b", false, false),
                fk("b", "a_id", "a", false, false),
            ],
        );
        assert!(report.has_cycles);
        assert!(
            report.breakable_edges.is_empty(),
            "no breakable edge ⇒ caller must surface DEFERRABLE-suggestion error"
        );
    }

    #[test]
    fn test_cycles_self_reference_is_not_a_cycle() {
        let report = detect_cycles(
            &tables(&["categories"]),
            &[fk("categories", "parent_id", "categories", true, false)],
        );
        assert!(!report.has_cycles);
        assert!(report.cycles.is_empty());
    }

    #[test]
    fn test_cycles_three_node_cycle() {
        // a → b → c → a (all non-nullable, non-deferrable)
        let report = detect_cycles(
            &tables(&["a", "b", "c"]),
            &[
                fk("a", "b_id", "b", false, false),
                fk("b", "c_id", "c", false, false),
                fk("c", "a_id", "a", false, false),
            ],
        );
        assert!(report.has_cycles);
        assert_eq!(report.cycles.len(), 1);
        assert_eq!(report.cycles[0], vec!["a", "b", "c"]);
        assert!(report.breakable_edges.is_empty());
    }

    #[test]
    fn test_cycles_normalized_to_smallest_first() {
        // Same cycle discovered starting from b: still normalizes to [a, b, c]
        let report = detect_cycles(
            &tables(&["b", "c", "a"]),
            &[
                fk("a", "b_id", "b", false, false),
                fk("b", "c_id", "c", false, false),
                fk("c", "a_id", "a", false, false),
            ],
        );
        assert_eq!(
            report.cycles,
            vec![vec!["a".to_string(), "b".into(), "c".into()]]
        );
    }
}
