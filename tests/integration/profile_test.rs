#![cfg(feature = "integration")]

//! Live-PostgreSQL tests for the profiling collector. Each test seeds known data
//! into the `schema_basic` fixture, profiles it, and asserts the captured
//! statistics — never any row-level values.

use std::env;
use std::fs;

use sqlx::PgPool;
use tokio::sync::Mutex;

use seedgen::introspection::introspect;
use seedgen::profile::{
    ColumnProfile, ComplianceCheck, ComplianceValidator, ProfileApplicator, ProfileCollector,
    ProfileOptions,
};
use seedgen::{generate, GenerateConfig};

// libtest runs these in parallel against a single PG instance; serialize at the
// binary level so schema apply + truncate + insert + profile can't interleave.
static SERIAL: Mutex<()> = Mutex::const_new(());

fn database_url() -> String {
    env::var("DATABASE_URL").expect("DATABASE_URL must be set for profile tests")
}

fn run<F, Fut, T>(f: F) -> T
where
    F: FnOnce(PgPool) -> Fut,
    Fut: std::future::Future<Output = T>,
{
    let rt = tokio::runtime::Runtime::new().expect("failed to build runtime");
    rt.block_on(async move {
        let _guard = SERIAL.lock().await;
        let pool = PgPool::connect(&database_url())
            .await
            .expect("connect failed");
        f(pool).await
    })
}

/// Apply the basic fixture only if its marker table is missing.
async fn ensure_basic(pool: &PgPool) {
    let (exists,): (bool,) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM information_schema.tables \
         WHERE table_schema = 'public' AND table_name = $1)",
    )
    .bind("posts")
    .fetch_one(pool)
    .await
    .expect("marker check failed");
    if !exists {
        sqlx::raw_sql("DROP SCHEMA IF EXISTS public CASCADE; CREATE SCHEMA public;")
            .execute(pool)
            .await
            .expect("schema reset failed");
        let sql = fs::read_to_string("tests/fixtures/schema_basic.sql").expect("read fixture");
        sqlx::raw_sql(&sql)
            .execute(pool)
            .await
            .expect("apply fixture");
    }
}

/// Insert a known, fully-controlled dataset:
///  - 100 users: name {Alice:50, Bob:30, Carol:20}; bio NULL for 40; is_active TRUE for 75;
///    100 unique emails.
///  - 500 posts (exactly 5 per user) → posts:users ratio 5.0.
///  - 1000 comments (2 per post).
async fn seed_known_data(pool: &PgPool) {
    sqlx::raw_sql("TRUNCATE users, posts, comments RESTART IDENTITY CASCADE")
        .execute(pool)
        .await
        .expect("truncate");
    sqlx::raw_sql(
        "INSERT INTO users (email, name, bio, is_active) \
         SELECT 'user' || g || '@example.com', \
                CASE WHEN g < 50 THEN 'Alice' WHEN g < 80 THEN 'Bob' ELSE 'Carol' END, \
                CASE WHEN g < 40 THEN NULL ELSE 'has bio' END, \
                (g < 75) \
         FROM generate_series(0, 99) AS g",
    )
    .execute(pool)
    .await
    .expect("insert users");
    sqlx::raw_sql(
        "INSERT INTO posts (user_id, title, slug, body) \
         SELECT u.id, 'title ' || u.id || '-' || s, 'slug-' || u.id || '-' || s, 'body' \
         FROM users u, generate_series(1, 5) AS s",
    )
    .execute(pool)
    .await
    .expect("insert posts");
    sqlx::raw_sql(
        "INSERT INTO comments (post_id, user_id, content) \
         SELECT p.id, p.user_id, 'comment' FROM posts p, generate_series(1, 2) AS s",
    )
    .execute(pool)
    .await
    .expect("insert comments");
}

#[test]
fn test_profile_row_counts_and_ratios() {
    run(|pool| async move {
        ensure_basic(&pool).await;
        seed_known_data(&pool).await;

        let schema = introspect(&pool).await.expect("introspect");
        let mut collector = ProfileCollector::new(&pool, schema, ProfileOptions::default())
            .await
            .expect("collector");
        let profile = collector.collect().await.expect("collect");

        assert_eq!(profile.tables["users"].row_count, 100);
        assert_eq!(profile.tables["posts"].row_count, 500);
        assert_eq!(profile.tables["comments"].row_count, 1000);

        let ratio = &profile.tables["posts"].parent_ratios["users"];
        assert!(
            (ratio.avg - 5.0).abs() < 0.1,
            "posts:users avg {}",
            ratio.avg
        );
        assert_eq!(ratio.min, 5);
        assert_eq!(ratio.max, 5);
    });
}

#[test]
fn test_profile_low_cardinality_captures_distribution() {
    run(|pool| async move {
        ensure_basic(&pool).await;
        seed_known_data(&pool).await;

        let schema = introspect(&pool).await.expect("introspect");
        let mut collector = ProfileCollector::new(&pool, schema, ProfileOptions::default())
            .await
            .expect("collector");
        let profile = collector.collect().await.expect("collect");

        // name → low cardinality → categorical with the seeded proportions.
        match &profile.tables["users"].columns["name"] {
            ColumnProfile::Categorical { distribution, .. } => {
                assert!(
                    (distribution["Alice"] - 50.0).abs() < 2.0,
                    "{distribution:?}"
                );
                assert!((distribution["Bob"] - 30.0).abs() < 2.0, "{distribution:?}");
                assert!(
                    (distribution["Carol"] - 20.0).abs() < 2.0,
                    "{distribution:?}"
                );
            }
            other => panic!("expected Categorical for name, got {other:?}"),
        }

        // is_active → boolean true_rate ~75%.
        match &profile.tables["users"].columns["is_active"] {
            ColumnProfile::Boolean { true_rate, .. } => {
                assert!((true_rate - 75.0).abs() < 2.0, "true_rate {true_rate}");
            }
            other => panic!("expected Boolean for is_active, got {other:?}"),
        }

        // bio → 40% NULL (single non-null value → captured as categorical).
        let bio_null = match &profile.tables["users"].columns["bio"] {
            ColumnProfile::Categorical { null_rate, .. } => *null_rate,
            ColumnProfile::StringStats { null_rate, .. } => *null_rate,
            other => panic!("unexpected bio profile {other:?}"),
        };
        assert!((bio_null - 40.0).abs() < 2.0, "bio null_rate {bio_null}");
    });
}

#[test]
fn test_profile_skips_sensitive_column() {
    run(|pool| async move {
        ensure_basic(&pool).await;
        seed_known_data(&pool).await;
        sqlx::raw_sql("ALTER TABLE users ADD COLUMN IF NOT EXISTS password_hash TEXT")
            .execute(&pool)
            .await
            .expect("add password column");

        let schema = introspect(&pool).await.expect("introspect");
        let mut collector = ProfileCollector::new(&pool, schema, ProfileOptions::default())
            .await
            .expect("collector");
        let profile = collector.collect().await.expect("collect");

        assert!(
            matches!(
                profile.tables["users"].columns["password_hash"],
                ColumnProfile::SkippedSensitive { .. }
            ),
            "password_hash should be skipped"
        );
        assert!(
            profile
                .options
                .skipped_sensitive
                .iter()
                .any(|s| s == "users.password_hash"),
            "skipped_sensitive summary missing password_hash: {:?}",
            profile.options.skipped_sensitive
        );
    });
}

#[test]
fn test_profile_high_cardinality_no_values_captured() {
    run(|pool| async move {
        ensure_basic(&pool).await;
        seed_known_data(&pool).await;

        let schema = introspect(&pool).await.expect("introspect");
        let mut collector = ProfileCollector::new(&pool, schema, ProfileOptions::default())
            .await
            .expect("collector");
        let profile = collector.collect().await.expect("collect");

        // 100 unique emails > threshold 50 → StringStats, NOT Categorical.
        match &profile.tables["users"].columns["email"] {
            ColumnProfile::StringStats { cardinality, .. } => {
                assert_eq!(*cardinality, 100);
            }
            other => panic!("expected StringStats for email, got {other:?}"),
        }

        // The profile must contain no actual email addresses.
        let yaml = serde_yaml::to_string(&profile).expect("serialize");
        assert!(!yaml.contains('@'), "profile leaked an email address");
        assert!(!yaml.contains("@example.com"));
    });
}

#[test]
fn test_profile_audit_log_complete() {
    run(|pool| async move {
        ensure_basic(&pool).await;
        seed_known_data(&pool).await;

        let schema = introspect(&pool).await.expect("introspect");
        let mut collector = ProfileCollector::new(&pool, schema, ProfileOptions::default())
            .await
            .expect("collector");
        let _ = collector.collect().await.expect("collect");

        let audit = collector.audit_log();
        assert!(audit.query_count() > 0, "no queries audited");
        // The serial `id` columns are recorded as skips.
        assert!(audit.skip_count() > 0, "no skips audited");

        let rendered = audit.render();
        assert!(rendered.contains("QUERY"));
        assert!(rendered.contains("SELECT COUNT(*)"));
        assert!(rendered.contains("SKIP"));
        assert!(!rendered.contains('@'), "audit log leaked an email address");

        // Persist and verify the file is written.
        let path =
            env::temp_dir().join(format!("seedgen-profile-audit-{}.log", std::process::id()));
        collector.write_audit_log(&path).expect("write audit log");
        let contents = fs::read_to_string(&path).expect("read audit log");
        assert!(!contents.is_empty());
        let _ = fs::remove_file(&path);
    });
}

#[test]
fn test_generate_and_export_queries_are_read_only() {
    run(|pool| async move {
        ensure_basic(&pool).await;

        let schema = introspect(&pool).await.expect("introspect");
        let collector = ProfileCollector::new(&pool, schema, ProfileOptions::default())
            .await
            .expect("collector");

        // Dry-run: queries are returned without executing.
        let planned = collector.generate_queries();
        assert!(!planned.is_empty());
        for q in &planned {
            assert!(q.sql.trim_start().to_uppercase().starts_with("SELECT"));
            assert!(!q.sql.contains("SELECT *"));
        }

        // Offline export: a single self-describing, read-only collection query.
        let sql_file = collector.export_queries();
        assert!(sql_file.contains("jsonb_build_object"));
        assert!(sql_file.contains("SELECT COUNT(*)"));
        for kw in ["INSERT", "UPDATE", "DELETE", "DROP"] {
            assert!(!sql_file.contains(kw), "export contained {kw}");
        }
    });
}

// ===========================================================================
// Applicator round-trip: profile → generate → verify (P.4)
// ===========================================================================

/// `n` users whose `name` follows a fixed 50/30/20 distribution; unique emails.
async fn seed_distribution_users(pool: &PgPool, n: i64) {
    sqlx::raw_sql("TRUNCATE users, posts, comments RESTART IDENTITY CASCADE")
        .execute(pool)
        .await
        .expect("truncate");
    sqlx::raw_sql(&format!(
        "INSERT INTO users (email, name, is_active) \
         SELECT 'u' || g || '@example.com', \
                CASE WHEN g % 10 < 5 THEN 'Alice' WHEN g % 10 < 8 THEN 'Bob' ELSE 'Carol' END, \
                true \
         FROM generate_series(0, {}) AS g",
        n - 1
    ))
    .execute(pool)
    .await
    .expect("insert users");
}

/// `n` users, each with exactly 5 posts (a clean 5.0 child:parent ratio).
async fn seed_users_with_posts(pool: &PgPool, n: i64) {
    sqlx::raw_sql("TRUNCATE users, posts, comments RESTART IDENTITY CASCADE")
        .execute(pool)
        .await
        .expect("truncate");
    sqlx::raw_sql(&format!(
        "INSERT INTO users (email, name, is_active) \
         SELECT 'u' || g || '@example.com', 'User ' || g, true \
         FROM generate_series(0, {}) AS g",
        n - 1
    ))
    .execute(pool)
    .await
    .expect("insert users");
    sqlx::raw_sql(
        "INSERT INTO posts (user_id, title, slug, body) \
         SELECT u.id, 'title ' || u.id || '-' || s, 'slug-' || u.id || '-' || s, 'body' \
         FROM users u, generate_series(1, 5) AS s",
    )
    .execute(pool)
    .await
    .expect("insert posts");
}

async fn name_distribution(pool: &PgPool) -> std::collections::HashMap<String, f64> {
    let rows: Vec<(String, f64)> = sqlx::query_as(
        "SELECT name, (COUNT(*) * 100.0 / SUM(COUNT(*)) OVER ())::float8 AS pct \
         FROM users GROUP BY name",
    )
    .fetch_all(pool)
    .await
    .expect("distribution query");
    rows.into_iter().collect()
}

async fn count_rows(pool: &PgPool, table: &str) -> i64 {
    let (n,): (i64,) = sqlx::query_as(&format!("SELECT COUNT(*) FROM {table}"))
        .fetch_one(pool)
        .await
        .expect("count query");
    n
}

#[test]
fn test_profile_roundtrip_distribution_preserved() {
    run(|pool| async move {
        ensure_basic(&pool).await;
        seed_distribution_users(&pool, 1000).await;

        let schema = introspect(&pool).await.expect("introspect");
        let mut collector = ProfileCollector::new(&pool, schema, ProfileOptions::default())
            .await
            .expect("collector");
        let profile = collector.collect().await.expect("collect");

        // Profile → scenario → regenerate (users only) at full scale, seed 42.
        let scenario = ProfileApplicator::new(profile, 1.0)
            .unwrap()
            .to_scenario()
            .unwrap();
        let config = GenerateConfig {
            seed: 42,
            rows_per_table: 10,
            scenario: Some(scenario),
            include_tables: Some(vec!["users".into()]),
            truncate_first: true,
            ..GenerateConfig::default()
        };
        generate(&pool, &config).await.expect("generate");

        let dist = name_distribution(&pool).await;
        assert!((dist["Alice"] - 50.0).abs() < 5.0, "Alice: {dist:?}");
        assert!((dist["Bob"] - 30.0).abs() < 5.0, "Bob: {dist:?}");
        assert!((dist["Carol"] - 20.0).abs() < 5.0, "Carol: {dist:?}");
    });
}

#[test]
fn test_profile_scale_preserves_ratios() {
    run(|pool| async move {
        ensure_basic(&pool).await;
        seed_users_with_posts(&pool, 200).await;

        let schema = introspect(&pool).await.expect("introspect");
        let mut collector = ProfileCollector::new(&pool, schema, ProfileOptions::default())
            .await
            .expect("collector");
        let profile = collector.collect().await.expect("collect");

        for scale in [1.0_f64, 0.1, 0.01] {
            let scenario = ProfileApplicator::new(profile.clone(), scale)
                .unwrap()
                .to_scenario()
                .unwrap();
            let config = GenerateConfig {
                seed: 42,
                rows_per_table: 10,
                scenario: Some(scenario),
                include_tables: Some(vec!["users".into(), "posts".into()]),
                truncate_first: true,
                ..GenerateConfig::default()
            };
            generate(&pool, &config).await.expect("generate");

            let users = count_rows(&pool, "users").await;
            let posts = count_rows(&pool, "posts").await;
            let expected_users = (200.0 * scale).round() as i64;
            assert_eq!(users, expected_users, "users at scale {scale}");
            assert!(users > 0, "no users at scale {scale}");
            let ratio = posts as f64 / users as f64;
            assert!(
                (ratio - 5.0).abs() < 0.5,
                "posts:users ratio {ratio} at scale {scale} (users={users}, posts={posts})"
            );
        }
    });
}

#[test]
fn test_profile_deterministic() {
    run(|pool| async move {
        ensure_basic(&pool).await;
        seed_distribution_users(&pool, 500).await;

        let schema = introspect(&pool).await.expect("introspect");
        let mut collector = ProfileCollector::new(&pool, schema, ProfileOptions::default())
            .await
            .expect("collector");
        let profile = collector.collect().await.expect("collect");
        let scenario = ProfileApplicator::new(profile, 1.0)
            .unwrap()
            .to_scenario()
            .unwrap();

        let config = GenerateConfig {
            seed: 123,
            rows_per_table: 10,
            scenario: Some(scenario),
            include_tables: Some(vec!["users".into()]),
            truncate_first: true,
            ..GenerateConfig::default()
        };

        generate(&pool, &config).await.expect("gen 1");
        let first: Vec<(i32, String)> = sqlx::query_as("SELECT id, name FROM users ORDER BY id")
            .fetch_all(&pool)
            .await
            .expect("query 1");

        generate(&pool, &config).await.expect("gen 2");
        let second: Vec<(i32, String)> = sqlx::query_as("SELECT id, name FROM users ORDER BY id")
            .fetch_all(&pool)
            .await
            .expect("query 2");

        assert!(!first.is_empty());
        assert_eq!(
            first, second,
            "same profile + seed must produce identical output"
        );
    });
}

// ===========================================================================
// FK integrity, privacy, and compliance (P.5)
// ===========================================================================

/// `n` users (name 50/30/20, ~50% active) with exactly one post each (ratio 1.0).
async fn seed_distribution_with_posts(pool: &PgPool, n: i64) {
    sqlx::raw_sql("TRUNCATE users, posts, comments RESTART IDENTITY CASCADE")
        .execute(pool)
        .await
        .expect("truncate");
    sqlx::raw_sql(&format!(
        "INSERT INTO users (email, name, is_active) \
         SELECT 'u' || g || '@example.com', \
                CASE WHEN g % 10 < 5 THEN 'Alice' WHEN g % 10 < 8 THEN 'Bob' ELSE 'Carol' END, \
                (g % 2 = 0) \
         FROM generate_series(0, {}) AS g",
        n - 1
    ))
    .execute(pool)
    .await
    .expect("insert users");
    sqlx::raw_sql(
        "INSERT INTO posts (user_id, title, slug, body) \
         SELECT u.id, 'title ' || u.id, 'slug-' || u.id, 'body' FROM users u",
    )
    .execute(pool)
    .await
    .expect("insert posts");
}

#[test]
fn test_profile_fk_integrity_maintained() {
    run(|pool| async move {
        ensure_basic(&pool).await;
        seed_users_with_posts(&pool, 200).await;

        let schema = introspect(&pool).await.expect("introspect");
        let mut collector = ProfileCollector::new(&pool, schema, ProfileOptions::default())
            .await
            .expect("collector");
        let profile = collector.collect().await.expect("collect");

        let scenario = ProfileApplicator::new(profile, 1.0)
            .unwrap()
            .to_scenario()
            .unwrap();
        let config = GenerateConfig {
            seed: 42,
            rows_per_table: 10,
            scenario: Some(scenario),
            include_tables: Some(vec!["users".into(), "posts".into()]),
            truncate_first: true,
            ..GenerateConfig::default()
        };
        generate(&pool, &config).await.expect("generate");

        // Every generated post must reference a real user.
        let (orphans,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM posts p \
             WHERE NOT EXISTS (SELECT 1 FROM users u WHERE u.id = p.user_id)",
        )
        .fetch_one(&pool)
        .await
        .expect("orphan query");
        assert_eq!(orphans, 0, "profile-based generation left orphaned FKs");
        assert!(count_rows(&pool, "posts").await > 0);
    });
}

#[test]
fn test_profile_contains_no_row_data() {
    run(|pool| async move {
        ensure_basic(&pool).await;
        // Distinctive sentinel buried in a high-cardinality column.
        sqlx::raw_sql("TRUNCATE users, posts, comments RESTART IDENTITY CASCADE")
            .execute(&pool)
            .await
            .expect("truncate");
        sqlx::raw_sql(
            "INSERT INTO users (email, name, is_active) \
             SELECT 'sentinel-zxcv-' || g || '@example.com', 'Person ' || g, true \
             FROM generate_series(0, 199) AS g",
        )
        .execute(&pool)
        .await
        .expect("insert users");

        let schema = introspect(&pool).await.expect("introspect");
        let mut collector = ProfileCollector::new(&pool, schema, ProfileOptions::default())
            .await
            .expect("collector");
        let profile = collector.collect().await.expect("collect");

        let yaml = serde_yaml::to_string(&profile).expect("serialize");
        // High-cardinality email/name values must NOT appear — stats only.
        assert!(
            !yaml.contains("sentinel-zxcv"),
            "row value leaked into profile"
        );
        assert!(!yaml.contains('@'), "email address leaked into profile");
        // But aggregate stats for those columns ARE present.
        assert!(yaml.contains("cardinality"));
    });
}

#[test]
fn test_profile_privacy_no_pii_in_yaml() {
    run(|pool| async move {
        ensure_basic(&pool).await;
        seed_known_data(&pool).await;
        // A sensitive column with realistic-looking PII that must be skipped.
        sqlx::raw_sql("ALTER TABLE users ADD COLUMN IF NOT EXISTS ssn TEXT")
            .execute(&pool)
            .await
            .expect("add ssn");
        sqlx::raw_sql("UPDATE users SET ssn = '123-45-' || LPAD((id % 10000)::text, 4, '0')")
            .execute(&pool)
            .await
            .expect("set ssn");

        let schema = introspect(&pool).await.expect("introspect");
        let mut collector = ProfileCollector::new(&pool, schema, ProfileOptions::default())
            .await
            .expect("collector");
        let profile = collector.collect().await.expect("collect");

        let yaml = serde_yaml::to_string(&profile).expect("serialize");
        assert!(!yaml.contains('@'), "email leaked");
        assert!(!yaml.contains("123-45-"), "SSN pattern leaked");
        assert!(
            profile
                .options
                .skipped_sensitive
                .iter()
                .any(|s| s == "users.ssn"),
            "ssn not recorded as skipped: {:?}",
            profile.options.skipped_sensitive
        );
    });
}

#[test]
fn test_profile_compliance_report() {
    run(|pool| async move {
        ensure_basic(&pool).await;
        seed_distribution_with_posts(&pool, 1000).await;

        let schema = introspect(&pool).await.expect("introspect");
        let mut collector = ProfileCollector::new(&pool, schema, ProfileOptions::default())
            .await
            .expect("collector");
        let profile = collector.collect().await.expect("collect");

        let scenario = ProfileApplicator::new(profile.clone(), 1.0)
            .unwrap()
            .to_scenario()
            .unwrap();
        let config = GenerateConfig {
            seed: 42,
            rows_per_table: 10,
            scenario: Some(scenario),
            include_tables: Some(vec!["users".into(), "posts".into()]),
            truncate_first: true,
            ..GenerateConfig::default()
        };
        generate(&pool, &config).await.expect("generate");

        let report = ComplianceValidator::with_default_tolerance(profile)
            .validate(&pool)
            .await
            .expect("validate");
        report.print();
        assert!(!report.checks.is_empty());

        // The distribution and ratio we explicitly reproduce must pass.
        let dist_ok = report.checks.iter().any(|c| {
            matches!(c,
                ComplianceCheck::Distribution { table, column, passed: true, .. }
                if table == "users" && column == "name")
        });
        assert!(dist_ok, "name distribution should pass: {report:?}");

        let ratio_ok = report.checks.iter().any(|c| {
            matches!(c,
                ComplianceCheck::Ratio { child_table, parent_table, passed: true, .. }
                if child_table == "posts" && parent_table == "users")
        });
        assert!(ratio_ok, "posts:users ratio should pass: {report:?}");
    });
}

// ===========================================================================
// Offline mode: export → simulate external run → import → matches live (P.6)
// ===========================================================================

#[test]
fn test_profile_offline_export_import_matches_live() {
    run(|pool| async move {
        ensure_basic(&pool).await;
        seed_known_data(&pool).await;

        let schema = introspect(&pool).await.expect("introspect");
        let mut collector = ProfileCollector::new(&pool, schema, ProfileOptions::default())
            .await
            .expect("collector");
        let live = collector.collect().await.expect("collect");

        // Simulate the DBA running the exported, self-describing collection query.
        let export_sql = collector.export_queries();
        let results: serde_json::Value = sqlx::query_scalar(&export_sql)
            .fetch_one(&pool)
            .await
            .expect("run export sql");
        let offline =
            seedgen::profile::import_results(&results.to_string()).expect("import results");

        assert_profiles_equivalent(&live, &offline);
    });
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

fn is_stat_column(c: &ColumnProfile) -> bool {
    !matches!(
        c,
        ColumnProfile::Serial
            | ColumnProfile::SkippedSensitive { .. }
            | ColumnProfile::SkippedExcluded
    )
}

fn column_close(a: &ColumnProfile, b: &ColumnProfile) -> bool {
    use ColumnProfile::*;
    match (a, b) {
        (
            Categorical {
                distribution: da,
                null_rate: na,
            },
            Categorical {
                distribution: db,
                null_rate: nb,
            },
        ) => {
            da.len() == db.len()
                && da
                    .iter()
                    .all(|(k, v)| db.get(k).map(|w| close(*v, *w)).unwrap_or(false))
                && close(*na, *nb)
        }
        (
            Numeric {
                min: a1,
                max: a2,
                mean: a3,
                median: a4,
                stddev: a5,
                null_rate: a6,
                ..
            },
            Numeric {
                min: b1,
                max: b2,
                mean: b3,
                median: b4,
                stddev: b5,
                null_rate: b6,
                ..
            },
        ) => {
            close(*a1, *b1)
                && close(*a2, *b2)
                && close(*a3, *b3)
                && close(*a4, *b4)
                && close(*a5, *b5)
                && close(*a6, *b6)
        }
        (
            Boolean {
                true_rate: a1,
                null_rate: a2,
            },
            Boolean {
                true_rate: b1,
                null_rate: b2,
            },
        ) => close(*a1, *b1) && close(*a2, *b2),
        (
            StringStats {
                cardinality: a1,
                null_rate: a2,
                avg_length: a3,
                ..
            },
            StringStats {
                cardinality: b1,
                null_rate: b2,
                avg_length: b3,
                ..
            },
        ) => a1 == b1 && close(*a2, *b2) && close(*a3, *b3),
        (
            Timestamp {
                range: a1,
                null_rate: a2,
                ..
            },
            Timestamp {
                range: b1,
                null_rate: b2,
                ..
            },
        ) => a1 == b1 && close(*a2, *b2),
        _ => false,
    }
}

/// Offline import must reproduce the live profile's statistics. (Serial/skipped
/// columns carry no stats and are not reconstructed offline, so they're ignored.)
fn assert_profiles_equivalent(
    live: &seedgen::profile::DatabaseProfile,
    offline: &seedgen::profile::DatabaseProfile,
) {
    assert_eq!(live.source_hash, offline.source_hash, "source_hash");
    assert_eq!(
        live.options.cardinality_threshold,
        offline.options.cardinality_threshold
    );
    assert_eq!(
        live.options.skipped_sensitive, offline.options.skipped_sensitive,
        "skipped_sensitive"
    );
    let live_tables: Vec<&String> = live.tables.keys().collect();
    let offline_tables: Vec<&String> = offline.tables.keys().collect();
    assert_eq!(live_tables, offline_tables, "table names");

    for (name, lt) in &live.tables {
        let ot = &offline.tables[name];
        assert_eq!(lt.row_count, ot.row_count, "{name} row_count");

        let lp: Vec<&String> = lt.parent_ratios.keys().collect();
        let op: Vec<&String> = ot.parent_ratios.keys().collect();
        assert_eq!(lp, op, "{name} parent_ratio keys");
        for (parent, lr) in &lt.parent_ratios {
            let or = &ot.parent_ratios[parent];
            assert!(close(lr.avg, or.avg), "{name}->{parent} avg");
            assert_eq!(lr.min, or.min, "{name}->{parent} min");
            assert_eq!(lr.max, or.max, "{name}->{parent} max");
            assert!(close(lr.median, or.median), "{name}->{parent} median");
        }

        for (col, lc) in &lt.columns {
            if is_stat_column(lc) {
                let oc = ot
                    .columns
                    .get(col)
                    .unwrap_or_else(|| panic!("{name}.{col} missing in offline profile"));
                assert!(column_close(lc, oc), "{name}.{col}: {lc:?} vs {oc:?}");
            }
        }
    }
}
