#![cfg(all(feature = "integration", feature = "proptest"))]

use std::env;
use std::fs;

use proptest::prelude::*;
use sqlx::PgPool;
use tokio::sync::Mutex;

use seedgen::{generate, GenerateConfig};

// libtest runs `proptest!`-generated `#[test]` fns in parallel threads against
// a single PG instance. We serialize at the binary level so schema apply +
// truncate + insert + select can't interleave between tests.
static SERIAL: Mutex<()> = Mutex::const_new(());

fn database_url() -> String {
    env::var("DATABASE_URL").expect("DATABASE_URL must be set for property tests")
}

fn build_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Runtime::new().expect("failed to build runtime")
}

/// Apply a fixture only if its marker table is missing. Cheap to call repeatedly.
async fn ensure_fixture(pool: &PgPool, marker_table: &str, fixture_path: &str) {
    let (exists,): (bool,) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM information_schema.tables \
         WHERE table_schema = 'public' AND table_name = $1)",
    )
    .bind(marker_table)
    .fetch_one(pool)
    .await
    .expect("marker check failed");
    if !exists {
        sqlx::raw_sql("DROP SCHEMA IF EXISTS public CASCADE; CREATE SCHEMA public;")
            .execute(pool)
            .await
            .expect("schema reset failed");
        let sql = fs::read_to_string(fixture_path)
            .unwrap_or_else(|e| panic!("read `{fixture_path}`: {e}"));
        sqlx::raw_sql(&sql)
            .execute(pool)
            .await
            .unwrap_or_else(|e| panic!("apply `{fixture_path}`: {e}"));
    }
}

async fn ensure_basic(pool: &PgPool) {
    ensure_fixture(pool, "posts", "tests/fixtures/schema_basic.sql").await;
}

async fn ensure_constrained(pool: &PgPool) {
    ensure_fixture(pool, "priced_items", "tests/fixtures/schema_with_check.sql").await;
}

async fn ensure_lifecycle(pool: &PgPool) {
    ensure_fixture(pool, "order_items", "tests/fixtures/schema_lifecycle.sql").await;
}

/// A small 5-month lifecycle scenario (root + child + grandchild) parametrized
/// by seed. Temporal constraints are on `created_at`, which the engine wires.
fn lifecycle_yaml(seed: u64) -> String {
    format!(
        r#"
seed: {seed}
lifecycle:
  start: 2024-01-01
  end: 2024-06-01
  bucket: month
tables:
  users:
    growth: {{ model: linear, initial: 8, rate: 3 }}
    churn: {{ rate: 0.15, grace_period: 2, column: is_active, value: false }}
  orders:
    growth: {{ follows: users, ratio: 1.5 }}
    temporal:
      created_at: {{ after: users.created_at, offset: 1d..20d }}
  order_items:
    growth: {{ follows: orders, per_parent: 1..3 }}
    temporal:
      created_at: {{ equals: orders.created_at }}
"#
    )
}

fn lifecycle_config(seed: u64) -> (GenerateConfig, String) {
    let yaml = lifecycle_yaml(seed);
    let scenario = seedgen::scenario::parse_scenario(&yaml).expect("parse lifecycle yaml");
    let config = GenerateConfig {
        seed,
        rows_per_table: 10,
        scenario: Some(scenario),
        truncate_first: true,
        ..GenerateConfig::default()
    };
    (config, yaml)
}

fn run<F, Fut, T>(f: F) -> T
where
    F: FnOnce(PgPool) -> Fut,
    Fut: std::future::Future<Output = T>,
{
    let rt = build_runtime();
    rt.block_on(async move {
        let _guard = SERIAL.lock().await;
        let pool = PgPool::connect(&database_url())
            .await
            .expect("connect failed");
        f(pool).await
    })
}

proptest! {
    // 12 cases per test * 6 tests = 72 DB round-trips. Each ~100ms = ~7s total.
    // Sufficient to catch most edge cases without making the suite painfully slow.
    #![proptest_config(ProptestConfig {
        cases: 12,
        max_shrink_iters: 32,
        ..ProptestConfig::default()
    })]

    /// NOT NULL columns must NEVER receive a NULL value.
    #[test]
    #[ignore]
    fn prop_no_null_in_not_null(seed in 0u64..1_000_000, rows in 1usize..30) {
        let violations: Vec<(String, i64)> = run(|pool| async move {
            ensure_basic(&pool).await;
            generate(&pool, &GenerateConfig {
                seed,
                rows_per_table: rows,
                truncate_first: true,
                ..GenerateConfig::default()
            }).await.expect("generate failed");

            let mut out = Vec::new();
            for (table, cols) in [
                ("users", &["email", "name", "is_active", "created_at"][..]),
                ("posts", &["user_id", "title", "slug"][..]),
                ("comments", &["post_id", "user_id", "content", "created_at"][..]),
            ] {
                for col in cols {
                    let (n,): (i64,) = sqlx::query_as(
                        &format!("SELECT COUNT(*) FROM {table} WHERE {col} IS NULL")
                    )
                    .fetch_one(&pool)
                    .await
                    .expect("count query failed");
                    if n > 0 {
                        out.push((format!("{table}.{col}"), n));
                    }
                }
            }
            out
        });

        prop_assert!(
            violations.is_empty(),
            "NULLs in NOT NULL columns: {:?} (seed={}, rows={})",
            violations, seed, rows
        );
    }

    /// Every FK value must reference an existing parent row.
    #[test]
    #[ignore]
    fn prop_fk_integrity(seed in 0u64..1_000_000) {
        let orphans = run(|pool| async move {
            ensure_basic(&pool).await;
            generate(&pool, &GenerateConfig {
                seed,
                rows_per_table: 15,
                truncate_first: true,
                ..GenerateConfig::default()
            }).await.expect("generate failed");

            let (posts_orphans,): (i64,) = sqlx::query_as(
                "SELECT COUNT(*) FROM posts p \
                 WHERE NOT EXISTS (SELECT 1 FROM users u WHERE u.id = p.user_id)"
            ).fetch_one(&pool).await.expect("query failed");

            let (comments_orphans,): (i64,) = sqlx::query_as(
                "SELECT COUNT(*) FROM comments c \
                 WHERE NOT EXISTS (SELECT 1 FROM posts p WHERE p.id = c.post_id) \
                    OR NOT EXISTS (SELECT 1 FROM users u WHERE u.id = c.user_id)"
            ).fetch_one(&pool).await.expect("query failed");

            posts_orphans + comments_orphans
        });

        prop_assert_eq!(orphans, 0, "FK violations found (seed={})", seed);
    }

    /// UNIQUE columns must contain no duplicates.
    #[test]
    #[ignore]
    fn prop_unique_no_duplicates(seed in 0u64..1_000_000, rows in 2usize..30) {
        let (email_dups, slug_dups) = run(|pool| async move {
            ensure_basic(&pool).await;
            generate(&pool, &GenerateConfig {
                seed,
                rows_per_table: rows,
                truncate_first: true,
                ..GenerateConfig::default()
            }).await.expect("generate failed");

            let (emails,): (i64,) = sqlx::query_as(
                "SELECT COUNT(*) FROM \
                   (SELECT email FROM users GROUP BY email HAVING COUNT(*) > 1) d"
            ).fetch_one(&pool).await.expect("query failed");

            let (slugs,): (i64,) = sqlx::query_as(
                "SELECT COUNT(*) FROM \
                   (SELECT slug FROM posts GROUP BY slug HAVING COUNT(*) > 1) d"
            ).fetch_one(&pool).await.expect("query failed");

            (emails, slugs)
        });

        prop_assert_eq!(email_dups, 0, "users.email duplicates (seed={}, rows={})", seed, rows);
        prop_assert_eq!(slug_dups, 0, "posts.slug duplicates (seed={}, rows={})", seed, rows);
    }

    /// generate(N) == generate(N) for any N — byte-for-byte determinism via PG state.
    #[test]
    #[ignore]
    fn prop_determinism(seed in 0u64..1_000_000) {
        type UserRow = (i32, String, String);
        let (run1, run2): (Vec<UserRow>, Vec<UserRow>) =
            run(|pool| async move {
                ensure_basic(&pool).await;
                let cfg = GenerateConfig {
                    seed,
                    rows_per_table: 12,
                    truncate_first: true,
                    ..GenerateConfig::default()
                };

                generate(&pool, &cfg).await.expect("first run failed");
                let r1 = sqlx::query_as(
                    "SELECT id, email, name FROM users ORDER BY id"
                ).fetch_all(&pool).await.expect("query failed");

                generate(&pool, &cfg).await.expect("second run failed");
                let r2 = sqlx::query_as(
                    "SELECT id, email, name FROM users ORDER BY id"
                ).fetch_all(&pool).await.expect("query failed");

                (r1, r2)
            });

        prop_assert_eq!(
            run1.clone(),
            run2.clone(),
            "two runs with seed={} differed: r1={:?}, r2={:?}",
            seed, run1, run2
        );
    }

    /// All generated emails must contain '@' and '.'.
    #[test]
    #[ignore]
    fn prop_email_format(seed in 0u64..1_000_000) {
        let emails: Vec<String> = run(|pool| async move {
            ensure_basic(&pool).await;
            generate(&pool, &GenerateConfig {
                seed,
                rows_per_table: 20,
                truncate_first: true,
                ..GenerateConfig::default()
            }).await.expect("generate failed");

            let rows: Vec<(String,)> = sqlx::query_as("SELECT email FROM users")
                .fetch_all(&pool).await.expect("query failed");
            rows.into_iter().map(|(e,)| e).collect()
        });

        for email in &emails {
            prop_assert!(
                email.contains('@') && email.contains('.'),
                "invalid email `{}` (seed={})", email, seed
            );
        }
    }

    /// All `price` values must be > 0 when CHECK(price > 0) is on the column.
    #[test]
    #[ignore]
    fn prop_money_positive(seed in 0u64..1_000_000) {
        let (rows_inserted, min_price, max_price) = run(|pool| async move {
            ensure_constrained(&pool).await;
            let result = generate(&pool, &GenerateConfig {
                seed,
                rows_per_table: 20,
                truncate_first: true,
                ..GenerateConfig::default()
            }).await.expect("generate failed (CHECK constraint may have been violated)");

            let (min, max): (f64, f64) = sqlx::query_as(
                "SELECT COALESCE(MIN(price), 1.0), COALESCE(MAX(price), 1.0) FROM priced_items"
            ).fetch_one(&pool).await.expect("query failed");

            (result.total_rows, min, max)
        });

        prop_assert!(rows_inserted > 0, "no rows inserted (seed={})", seed);
        prop_assert!(min_price > 0.0, "min price {} not > 0 (seed={})", min_price, seed);
        prop_assert!(max_price > 0.0, "max price {} not > 0 (seed={})", max_price, seed);
    }
}

proptest! {
    // Lifecycle generation is heavier (multi-bucket, multi-table) than a single
    // generate, so use fewer cases. Seeds are sampled across the full range.
    #![proptest_config(ProptestConfig {
        cases: 8,
        max_shrink_iters: 16,
        ..ProptestConfig::default()
    })]

    /// No child row may be timestamped before its parent: orders after their
    /// user, order_items at-or-after their order.
    #[test]
    #[ignore]
    fn prop_lifecycle_temporal_consistency(seed in 0u64..100_000) {
        let (orders_before_user, items_before_order) = run(|pool| async move {
            ensure_lifecycle(&pool).await;
            let (config, _) = lifecycle_config(seed);
            generate(&pool, &config).await.expect("lifecycle generate failed");

            let (a,): (i64,) = sqlx::query_as(
                "SELECT COUNT(*) FROM orders o JOIN users u ON o.user_id = u.id \
                 WHERE o.created_at < u.created_at"
            ).fetch_one(&pool).await.expect("query failed");

            let (b,): (i64,) = sqlx::query_as(
                "SELECT COUNT(*) FROM order_items i JOIN orders o ON i.order_id = o.id \
                 WHERE i.created_at < o.created_at"
            ).fetch_one(&pool).await.expect("query failed");

            (a, b)
        });

        prop_assert_eq!(orders_before_user, 0, "orders before their user (seed={})", seed);
        prop_assert_eq!(items_before_order, 0, "order_items before their order (seed={})", seed);
    }

    /// Churned users can never exceed total users, and every churned user is
    /// stamped — churn only flips existing rows, never invents them.
    #[test]
    #[ignore]
    fn prop_lifecycle_churn_bounded(seed in 0u64..100_000) {
        let (total, churned, unstamped) = run(|pool| async move {
            ensure_lifecycle(&pool).await;
            let (config, _) = lifecycle_config(seed);
            generate(&pool, &config).await.expect("lifecycle generate failed");

            let (t,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
                .fetch_one(&pool).await.expect("query failed");
            let (c,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users WHERE NOT is_active")
                .fetch_one(&pool).await.expect("query failed");
            let (u,): (i64,) = sqlx::query_as(
                "SELECT COUNT(*) FROM users WHERE NOT is_active AND churned_at IS NULL"
            ).fetch_one(&pool).await.expect("query failed");

            (t, c, u)
        });

        prop_assert!(churned <= total, "churned {} > total {} (seed={})", churned, total, seed);
        prop_assert_eq!(unstamped, 0, "churned users without a timestamp (seed={})", seed);
    }

    /// Every FK value references an existing parent row (zero orphans).
    #[test]
    #[ignore]
    fn prop_lifecycle_fk_integrity(seed in 0u64..100_000) {
        let orphans = run(|pool| async move {
            ensure_lifecycle(&pool).await;
            let (config, _) = lifecycle_config(seed);
            generate(&pool, &config).await.expect("lifecycle generate failed");

            let (order_orphans,): (i64,) = sqlx::query_as(
                "SELECT COUNT(*) FROM orders o \
                 WHERE NOT EXISTS (SELECT 1 FROM users u WHERE u.id = o.user_id)"
            ).fetch_one(&pool).await.expect("query failed");

            let (item_orphans,): (i64,) = sqlx::query_as(
                "SELECT COUNT(*) FROM order_items i \
                 WHERE NOT EXISTS (SELECT 1 FROM orders o WHERE o.id = i.order_id)"
            ).fetch_one(&pool).await.expect("query failed");

            order_orphans + item_orphans
        });

        prop_assert_eq!(orphans, 0, "lifecycle FK violations (seed={})", seed);
    }
}
