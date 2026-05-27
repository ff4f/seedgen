#![cfg(feature = "integration")]

use std::collections::BTreeMap;
use std::env;
use std::fs;

use serde::Serialize;
use sqlx::PgPool;
use tokio::sync::Mutex;

use seedgen::{generate, GenerateConfig};

// Serialize all snapshot tests in this binary against the shared DB.
static SERIAL: Mutex<()> = Mutex::const_new(());

fn database_url() -> String {
    env::var("DATABASE_URL").expect("DATABASE_URL must be set for snapshot tests")
}

/// Apply schema_basic.sql only if the marker table is missing — keeps repeated
/// runs fast and doesn't fight with other integration test binaries.
async fn ensure_basic(pool: &PgPool) {
    let (exists,): (bool,) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM information_schema.tables \
         WHERE table_schema = 'public' AND table_name = 'posts')",
    )
    .fetch_one(pool)
    .await
    .expect("marker check failed");
    if !exists {
        sqlx::raw_sql("DROP SCHEMA IF EXISTS public CASCADE; CREATE SCHEMA public;")
            .execute(pool)
            .await
            .expect("schema reset failed");
        let fixture = fs::read_to_string("tests/fixtures/schema_basic.sql").expect("read fixture");
        sqlx::raw_sql(&fixture)
            .execute(pool)
            .await
            .expect("apply schema_basic.sql");
    }
}

type Row = BTreeMap<String, serde_json::Value>;

#[derive(Debug, Serialize)]
struct Snapshot {
    seed: u64,
    rows_per_table: usize,
    tables: BTreeMap<String, Vec<Row>>,
}

async fn capture(seed: u64, rows_per_table: usize) -> Snapshot {
    let _guard = SERIAL.lock().await;
    let pool = PgPool::connect(&database_url())
        .await
        .expect("connect failed");
    ensure_basic(&pool).await;

    let config = GenerateConfig {
        seed,
        rows_per_table,
        truncate_first: true,
        ..GenerateConfig::default()
    };
    generate(&pool, &config).await.expect("generate failed");

    let mut tables: BTreeMap<String, Vec<Row>> = BTreeMap::new();
    for table in ["users", "posts", "comments"] {
        // row_to_json gives us a column-name-keyed object per row, dodging the
        // need to know each column's Rust type up-front.
        let json_rows: Vec<(serde_json::Value,)> = sqlx::query_as(&format!(
            "SELECT row_to_json({table}) FROM {table} ORDER BY id"
        ))
        .fetch_all(&pool)
        .await
        .unwrap_or_else(|e| panic!("query {table}: {e}"));

        let rows: Vec<Row> = json_rows
            .into_iter()
            .map(|(v,)| match v {
                // Re-collect into a BTreeMap so column order is alphabetical and
                // therefore byte-stable regardless of serde_json's `preserve_order`.
                serde_json::Value::Object(m) => m.into_iter().collect(),
                other => panic!("expected JSON object per row, got {other:?}"),
            })
            .collect();
        tables.insert(table.to_string(), rows);
    }

    Snapshot {
        seed,
        rows_per_table,
        tables,
    }
}

#[tokio::test]
async fn snapshot_basic_seed_42() {
    let data = capture(42, 5).await;
    insta::assert_yaml_snapshot!("basic_seed_42", &data);
}

#[tokio::test]
async fn snapshot_basic_seed_7() {
    // Second snapshot at a different seed proves the determinism contract holds
    // for at least two seeds, and that different seeds produce different output.
    let data = capture(7, 5).await;
    insta::assert_yaml_snapshot!("basic_seed_7", &data);
}
