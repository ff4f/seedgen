//! Determinism snapshot for lifecycle simulation. Uses the in-memory dry-run
//! (`LifecycleEngine::simulate`) so it needs no database: it locks the per-bucket
//! growth/churn/seasonality counts for a fixed seed. If a change alters the
//! numbers for the same seed, this snapshot fails — that's the determinism
//! contract (a deliberate change requires re-accepting the snapshot).

use std::collections::BTreeMap;

use serde::Serialize;

use seedgen::lifecycle::LifecycleEngine;
use seedgen::scenario::parse_scenario;

#[derive(Serialize)]
struct StatSnapshot {
    table: String,
    new: usize,
    churned: usize,
    active: usize,
}

#[derive(Serialize)]
struct BucketSnapshot {
    label: String,
    stats: Vec<StatSnapshot>,
}

#[derive(Serialize)]
struct SimulationSnapshot {
    seed: u64,
    table_order: Vec<String>,
    buckets: Vec<BucketSnapshot>,
    // BTreeMap so key order is deterministic in the snapshot.
    totals: BTreeMap<String, (usize, usize)>,
}

#[test]
fn lifecycle_simulation_seed_42() {
    // Simple 6-month scenario, 3 tables: a churning root, a seasonal child, and
    // a per-parent grandchild.
    let yaml = r#"
seed: 42
lifecycle:
  start: 2024-01-01
  end: 2024-07-01
  bucket: month
tables:
  users:
    growth: { model: s_curve, initial: 10, capacity: 200, rate: 0.4 }
    churn: { rate: 0.1, grace_period: 2, column: is_active, value: false }
  orders:
    growth: { follows: users, ratio: 2.0 }
    seasonality:
      monthly: [1.0, 0.8, 1.0, 1.2, 1.0, 1.5, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0]
  order_items:
    growth: { follows: orders, per_parent: 1..3 }
"#;

    let scenario = parse_scenario(yaml).expect("parse failed");
    let lifecycle = scenario.lifecycle.expect("lifecycle block present");
    let engine = LifecycleEngine::new(lifecycle, scenario.table_lifecycles);
    let report = engine.simulate(42);

    let snapshot = SimulationSnapshot {
        seed: 42,
        table_order: report.table_order.clone(),
        buckets: report
            .buckets
            .iter()
            .map(|b| BucketSnapshot {
                label: b.label.clone(),
                stats: b
                    .stats
                    .iter()
                    .map(|s| StatSnapshot {
                        table: s.table.clone(),
                        new: s.new,
                        churned: s.churned,
                        active: s.active,
                    })
                    .collect(),
            })
            .collect(),
        totals: report.totals.iter().map(|(k, v)| (k.clone(), *v)).collect(),
    };

    insta::assert_yaml_snapshot!("lifecycle_simulation_seed_42", &snapshot);
}
