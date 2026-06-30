#![cfg(feature = "integration")]

//! Snapshot test that locks the shape of a profile built from deterministic
//! (seed 42) generated data over `schema_basic`.

use std::env;
use std::fs;

use sqlx::PgPool;
use tokio::sync::Mutex;

use seedgen::introspection::introspect;
use seedgen::profile::{ProfileCollector, ProfileOptions};
use seedgen::{generate, GenerateConfig};

static SERIAL: Mutex<()> = Mutex::const_new(());

fn database_url() -> String {
    env::var("DATABASE_URL").expect("DATABASE_URL must be set for snapshot tests")
}

/// Always reset to a pristine `schema_basic` — other test binaries mutate the
/// shared schema (e.g. adding `password_hash`/`ssn`), which would otherwise leak
/// extra columns into this snapshot.
async fn reset_basic(pool: &PgPool) {
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

#[tokio::test]
async fn snapshot_profile_basic_seed_42() {
    let _guard = SERIAL.lock().await;
    let pool = PgPool::connect(&database_url())
        .await
        .expect("connect failed");
    reset_basic(&pool).await;

    // Deterministic data: seed 42 produces byte-identical rows every run, so the
    // profile of that data is itself deterministic.
    let config = GenerateConfig {
        seed: 42,
        rows_per_table: 50,
        truncate_first: true,
        ..GenerateConfig::default()
    };
    generate(&pool, &config).await.expect("generate failed");

    let schema = introspect(&pool).await.expect("introspect");
    let mut collector = ProfileCollector::new(&pool, schema, ProfileOptions::default())
        .await
        .expect("collector");
    let mut profile = collector.collect().await.expect("collect failed");

    // Normalize fields that aren't data-deterministic (wall clock, crate version).
    profile.profiled_at = "[normalized]".to_string();
    profile.seedgen_version = "[normalized]".to_string();

    insta::assert_yaml_snapshot!("profile_basic_seed_42", &profile);
}
