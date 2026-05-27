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
