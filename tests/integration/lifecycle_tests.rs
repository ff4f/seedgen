#![cfg(feature = "integration")]

//! Integration tests for lifecycle (time-travel) generation. These run against a
//! live PostgreSQL instance (DATABASE_URL) and exercise the full pipeline:
//! parse → introspect → resolve → LifecycleEngine.execute.

use std::env;
use std::fs;

use seedgen::scenario::parse_scenario;
use seedgen::{generate, GenerateConfig, GenerationResult, OutputMode};
use sqlx::{PgPool, Row};
use tokio::sync::Mutex;

/// Serializes DB-mutating tests: each test resets the schema, so they must not
/// run concurrently against the shared database.
static DB_LOCK: Mutex<()> = Mutex::const_new(());

fn database_url() -> String {
    env::var("DATABASE_URL").expect("DATABASE_URL must be set for integration tests")
}

/// Drop everything and reload the lifecycle fixture schema.
async fn reset_schema(pool: &PgPool) {
    sqlx::raw_sql("DROP SCHEMA IF EXISTS public CASCADE; CREATE SCHEMA public;")
        .execute(pool)
        .await
        .expect("failed to reset public schema");
    let fixture = fs::read_to_string("tests/fixtures/schema_lifecycle.sql")
        .expect("failed to read schema_lifecycle.sql");
    sqlx::raw_sql(&fixture)
        .execute(pool)
        .await
        .expect("failed to apply schema_lifecycle.sql");
}

async fn run_lifecycle(pool: &PgPool, yaml: &str, seed: u64) -> GenerationResult {
    let scenario = parse_scenario(yaml).expect("failed to parse scenario");
    let config = GenerateConfig {
        seed,
        rows_per_table: 10,
        scenario: Some(scenario),
        output_mode: OutputMode::DirectInsert,
        include_tables: None,
        exclude_tables: None,
        truncate_first: false,
    };
    generate(pool, &config).await.expect("generation failed")
}

async fn scalar_i64(pool: &PgPool, sql: &str) -> i64 {
    sqlx::query_scalar(sql)
        .fetch_one(pool)
        .await
        .unwrap_or_else(|e| panic!("query failed `{sql}`: {e}"))
}

// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_lifecycle_temporal_consistency_orders_after_users() {
    let _guard = DB_LOCK.lock().await;
    let pool = PgPool::connect(&database_url()).await.expect("connect");
    reset_schema(&pool).await;

    let yaml = r#"
seed: 42
lifecycle:
  start: 2024-01-01
  end: 2024-07-01
  bucket: month
tables:
  users:
    growth: { model: linear, initial: 8, rate: 4 }
  orders:
    growth: { follows: users, ratio: 2.0 }
    temporal:
      created_at: { after: users.created_at, offset: 1d..30d }
"#;
    run_lifecycle(&pool, yaml, 42).await;

    let orders = scalar_i64(&pool, "SELECT COUNT(*) FROM orders").await;
    assert!(orders > 0, "expected some orders to be generated");

    let violations = scalar_i64(
        &pool,
        "SELECT COUNT(*) FROM orders o JOIN users u ON o.user_id = u.id \
         WHERE o.created_at < u.created_at",
    )
    .await;
    assert_eq!(violations, 0, "found orders created before their user");
}

#[tokio::test]
async fn test_lifecycle_growth_is_increasing() {
    let _guard = DB_LOCK.lock().await;
    let pool = PgPool::connect(&database_url()).await.expect("connect");
    reset_schema(&pool).await;

    // Exponential, rate 1.0 (doubling): per-bucket new counts are 5,5,10,20,40,80.
    let yaml = r#"
seed: 7
lifecycle:
  start: 2024-01-01
  end: 2024-07-01
  bucket: month
tables:
  users:
    growth: { model: exponential, initial: 5, rate: 1.0 }
"#;
    run_lifecycle(&pool, yaml, 7).await;

    let rows = sqlx::query(
        "SELECT TO_CHAR(DATE_TRUNC('month', created_at), 'YYYY-MM') AS m, COUNT(*) AS c \
         FROM users GROUP BY 1 ORDER BY 1",
    )
    .fetch_all(&pool)
    .await
    .expect("monthly query");

    let counts: Vec<i64> = rows.iter().map(|r| r.get::<i64, _>("c")).collect();
    assert!(counts.len() >= 3, "expected several months, got {counts:?}");

    // Trend upward over every 3-month window.
    for w in counts.windows(3) {
        assert!(
            w[2] >= w[0],
            "growth should trend upward across 3-month windows: {counts:?}"
        );
    }
    // And overall: the last month exceeds the first.
    assert!(
        counts.last().unwrap() > counts.first().unwrap(),
        "last month should exceed the first: {counts:?}"
    );
}

#[tokio::test]
async fn test_lifecycle_churn_marks_inactive() {
    let _guard = DB_LOCK.lock().await;
    let pool = PgPool::connect(&database_url()).await.expect("connect");
    reset_schema(&pool).await;

    let yaml = r#"
seed: 99
lifecycle:
  start: 2024-01-01
  end: 2024-12-01
  bucket: month
tables:
  users:
    growth: { model: linear, initial: 30, rate: 10 }
    churn: { rate: 0.2, grace_period: 3, column: is_active, value: false }
  orders:
    growth: { follows: users, ratio: 1.5 }
    temporal:
      created_at: { after: users.created_at, offset: 1d..20d }
"#;
    run_lifecycle(&pool, yaml, 99).await;

    let active = scalar_i64(&pool, "SELECT COUNT(*) FROM users WHERE is_active").await;
    let churned = scalar_i64(&pool, "SELECT COUNT(*) FROM users WHERE NOT is_active").await;
    assert!(active > 0, "expected some active users");
    assert!(
        churned > 0,
        "expected some churned users (is_active = false)"
    );

    // Cascade: once a user churns it is removed from the FK pool, so it gets no
    // orders after its churn time. Every churned user has a churn timestamp.
    let unstamped = scalar_i64(
        &pool,
        "SELECT COUNT(*) FROM users WHERE NOT is_active AND churned_at IS NULL",
    )
    .await;
    assert_eq!(
        unstamped, 0,
        "churned users should have a churned_at timestamp"
    );

    let post_churn_orders = scalar_i64(
        &pool,
        "SELECT COUNT(*) FROM orders o JOIN users u ON o.user_id = u.id \
         WHERE u.churned_at IS NOT NULL AND o.created_at > u.churned_at",
    )
    .await;
    assert_eq!(
        post_churn_orders, 0,
        "churned users should have no orders created after their churn time"
    );
}

#[tokio::test]
async fn test_lifecycle_seasonality_december_peak() {
    let _guard = DB_LOCK.lock().await;
    let pool = PgPool::connect(&database_url()).await.expect("connect");
    reset_schema(&pool).await;

    let yaml = r#"
seed: 5
lifecycle:
  start: 2024-01-01
  end: 2025-01-01
  bucket: month
tables:
  users:
    growth: { model: linear, initial: 40, rate: 6 }
  orders:
    growth: { follows: users, ratio: 1.0 }
    seasonality:
      monthly: [1.0, 0.7, 0.85, 1.0, 1.1, 0.8, 0.7, 0.85, 1.2, 1.4, 1.8, 2.5]
    temporal:
      created_at: { after: users.created_at, offset: 1d..15d }
"#;
    run_lifecycle(&pool, yaml, 5).await;

    let rows = sqlx::query(
        "SELECT EXTRACT(MONTH FROM created_at)::int AS m, COUNT(*) AS c \
         FROM orders GROUP BY 1 ORDER BY 2 DESC",
    )
    .fetch_all(&pool)
    .await
    .expect("monthly order query");
    assert!(!rows.is_empty(), "expected orders");

    let top_month: i32 = rows[0].get("m");
    assert_eq!(top_month, 12, "December (2.5x) should have the most orders");
}

#[tokio::test]
async fn test_lifecycle_fk_integrity_maintained() {
    let _guard = DB_LOCK.lock().await;
    let pool = PgPool::connect(&database_url()).await.expect("connect");
    reset_schema(&pool).await;

    let yaml = r#"
seed: 13
lifecycle:
  start: 2024-01-01
  end: 2024-06-01
  bucket: month
tables:
  users:
    growth: { model: linear, initial: 10, rate: 5 }
  orders:
    growth: { follows: users, ratio: 2.0 }
    temporal:
      created_at: { after: users.created_at, offset: 1d..20d }
  order_items:
    growth: { follows: orders, per_parent: 1..4 }
    temporal:
      created_at: { equals: orders.created_at }
"#;
    run_lifecycle(&pool, yaml, 13).await;

    let order_orphans = scalar_i64(
        &pool,
        "SELECT COUNT(*) FROM orders o LEFT JOIN users u ON o.user_id = u.id \
         WHERE u.id IS NULL",
    )
    .await;
    assert_eq!(order_orphans, 0, "every order.user_id must exist in users");

    let item_orphans = scalar_i64(
        &pool,
        "SELECT COUNT(*) FROM order_items i LEFT JOIN orders o ON i.order_id = o.id \
         WHERE o.id IS NULL",
    )
    .await;
    assert_eq!(
        item_orphans, 0,
        "every order_item.order_id must exist in orders"
    );
}

#[tokio::test]
async fn test_lifecycle_determinism() {
    let _guard = DB_LOCK.lock().await;
    let pool = PgPool::connect(&database_url()).await.expect("connect");

    let yaml = r#"
seed: 21
lifecycle:
  start: 2024-01-01
  end: 2024-08-01
  bucket: month
tables:
  users:
    growth: { model: exponential, initial: 6, rate: 0.4 }
    churn: { rate: 0.1, grace_period: 2, column: is_active, value: false }
  orders:
    growth: { follows: users, ratio: 2.0, variance: 0.3 }
    temporal:
      created_at: { after: users.created_at, offset: 1d..20d }
"#;

    reset_schema(&pool).await;
    let first = run_lifecycle(&pool, yaml, 21).await;

    reset_schema(&pool).await;
    let second = run_lifecycle(&pool, yaml, 21).await;

    let per_table = |r: &GenerationResult| -> Vec<(String, usize)> {
        let mut v: Vec<(String, usize)> = r
            .tables_seeded
            .iter()
            .map(|t| (t.name.clone(), t.rows_inserted))
            .collect();
        v.sort();
        v
    };

    assert_eq!(
        per_table(&first),
        per_table(&second),
        "same seed must produce identical per-table row counts"
    );
    assert_eq!(first.total_rows, second.total_rows);
}
