#![cfg(feature = "integration")]

use std::env;
use std::fs;

use seedgen::output::{insert_rows, truncate_tables};
use seedgen::{generate, GenerateConfig};
use sqlx::PgPool;
use tokio::sync::{Mutex, OnceCell};

static SETUP: OnceCell<()> = OnceCell::const_new();
// libtest runs `#[tokio::test]` functions in parallel; since they share one DB,
// we serialize them at the binary level so seq/truncate/insert ops can't interleave.
static SERIAL: Mutex<()> = Mutex::const_new(());

fn database_url() -> String {
    env::var("DATABASE_URL").expect("DATABASE_URL must be set for integration tests")
}

async fn ensure_fixture_applied() {
    SETUP
        .get_or_init(|| async {
            let pool = PgPool::connect(&database_url())
                .await
                .expect("failed to connect to database");
            sqlx::raw_sql("DROP SCHEMA IF EXISTS public CASCADE; CREATE SCHEMA public;")
                .execute(&pool)
                .await
                .expect("failed to reset public schema");
            let fixture = fs::read_to_string("tests/fixtures/schema_basic.sql")
                .expect("failed to read tests/fixtures/schema_basic.sql");
            sqlx::raw_sql(&fixture)
                .execute(&pool)
                .await
                .expect("failed to apply schema_basic.sql");
            pool.close().await;
        })
        .await;
}

async fn fresh_pool() -> PgPool {
    ensure_fixture_applied().await;
    PgPool::connect(&database_url())
        .await
        .expect("failed to connect to database")
}

async fn count(pool: &PgPool, table: &str) -> i64 {
    let row: (i64,) = sqlx::query_as(&format!("SELECT COUNT(*) FROM {table}"))
        .fetch_one(pool)
        .await
        .unwrap_or_else(|e| panic!("count {table} failed: {e}"));
    row.0
}

#[tokio::test]
async fn test_generate_inserts_ten_rows_per_table() {
    let _guard = SERIAL.lock().await;
    let pool = fresh_pool().await;

    let config = GenerateConfig {
        seed: 42,
        rows_per_table: 10,
        truncate_first: true,
        ..GenerateConfig::default()
    };

    let result = generate(&pool, &config).await.expect("generate failed");

    assert_eq!(result.total_rows, 30, "expected 30 rows total");
    assert_eq!(result.tables_seeded.len(), 3);
    assert_eq!(result.seed_used, 42);

    assert_eq!(count(&pool, "users").await, 10);
    assert_eq!(count(&pool, "posts").await, 10);
    assert_eq!(count(&pool, "comments").await, 10);
}

#[tokio::test]
async fn test_generate_respects_fk_integrity() {
    let _guard = SERIAL.lock().await;
    let pool = fresh_pool().await;

    let config = GenerateConfig {
        seed: 7,
        rows_per_table: 10,
        truncate_first: true,
        ..GenerateConfig::default()
    };
    generate(&pool, &config).await.expect("generate failed");

    // Every posts.user_id must point to a real users.id.
    let orphan_posts: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM posts p WHERE NOT EXISTS \
         (SELECT 1 FROM users u WHERE u.id = p.user_id)",
    )
    .fetch_one(&pool)
    .await
    .expect("query failed");
    assert_eq!(orphan_posts.0, 0, "found posts with orphan user_id");

    let orphan_comments: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM comments c \
         WHERE NOT EXISTS (SELECT 1 FROM posts p WHERE p.id = c.post_id) \
            OR NOT EXISTS (SELECT 1 FROM users u WHERE u.id = c.user_id)",
    )
    .fetch_one(&pool)
    .await
    .expect("query failed");
    assert_eq!(orphan_comments.0, 0, "found comments with orphan FKs");
}

#[tokio::test]
async fn test_truncate_tables_clears_all_rows() {
    let _guard = SERIAL.lock().await;
    let pool = fresh_pool().await;

    // Seed first.
    let config = GenerateConfig {
        seed: 1,
        rows_per_table: 5,
        truncate_first: true,
        ..GenerateConfig::default()
    };
    generate(&pool, &config).await.expect("generate failed");
    assert!(count(&pool, "users").await > 0);

    // Reverse topo order: children first, but CASCADE handles deps either way.
    let tables = vec![
        "comments".to_string(),
        "posts".to_string(),
        "users".to_string(),
    ];
    truncate_tables(&pool, &tables)
        .await
        .expect("truncate failed");

    assert_eq!(count(&pool, "users").await, 0);
    assert_eq!(count(&pool, "posts").await, 0);
    assert_eq!(count(&pool, "comments").await, 0);
}

#[tokio::test]
async fn test_truncate_tables_resets_identity() {
    let _guard = SERIAL.lock().await;
    let pool = fresh_pool().await;

    // Seed, truncate, seed again — IDs should restart from 1.
    let config = GenerateConfig {
        seed: 1,
        rows_per_table: 3,
        truncate_first: false,
        ..GenerateConfig::default()
    };

    truncate_tables(&pool, &["comments".into(), "posts".into(), "users".into()])
        .await
        .expect("first truncate failed");
    generate(&pool, &config)
        .await
        .expect("first generate failed");

    truncate_tables(&pool, &["comments".into(), "posts".into(), "users".into()])
        .await
        .expect("second truncate failed");
    generate(&pool, &config)
        .await
        .expect("second generate failed");

    let min_id: (i32,) = sqlx::query_as("SELECT MIN(id) FROM users")
        .fetch_one(&pool)
        .await
        .expect("min query failed");
    assert_eq!(
        min_id.0, 1,
        "TRUNCATE RESTART IDENTITY should reset the sequence"
    );
}

#[tokio::test]
async fn test_insert_rows_returns_generated_ids() {
    use seedgen::generators::Value;

    let _guard = SERIAL.lock().await;
    let pool = fresh_pool().await;
    truncate_tables(&pool, &["comments".into(), "posts".into(), "users".into()])
        .await
        .expect("truncate failed");

    let columns = vec!["email".to_string(), "name".to_string()];
    let rows = vec![
        vec![
            Value::String("alice@example.com".into()),
            Value::String("Alice".into()),
        ],
        vec![
            Value::String("bob@example.com".into()),
            Value::String("Bob".into()),
        ],
        vec![
            Value::String("carol@example.com".into()),
            Value::String("Carol".into()),
        ],
    ];

    let ids = insert_rows(&pool, "users", &columns, &rows)
        .await
        .expect("insert_rows failed");

    assert_eq!(ids.len(), 3);
    // SERIAL starts at 1, monotonically increasing.
    assert!(
        ids[0] < ids[1] && ids[1] < ids[2],
        "ids not monotonic: {ids:?}"
    );
    assert_eq!(count(&pool, "users").await, 3);
}

#[tokio::test]
async fn test_insert_rows_rejects_shape_mismatch() {
    use seedgen::generators::Value;
    use seedgen::output::OutputError;

    let _guard = SERIAL.lock().await;
    let pool = fresh_pool().await;
    truncate_tables(&pool, &["comments".into(), "posts".into(), "users".into()])
        .await
        .expect("truncate failed");

    let columns = vec!["email".to_string(), "name".to_string()];
    let bad_rows = vec![vec![Value::String("only-one".into())]];

    let err = insert_rows(&pool, "users", &columns, &bad_rows)
        .await
        .expect_err("should reject shape mismatch");
    assert!(
        matches!(err, OutputError::ShapeMismatch { .. }),
        "got {err:?}"
    );
}
