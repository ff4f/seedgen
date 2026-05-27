#![cfg(feature = "integration")]

use std::env;
use std::fs;

use seedgen::introspection::introspect;
use seedgen::resolver::resolve;
use sqlx::PgPool;
use tokio::sync::OnceCell;

static SETUP: OnceCell<()> = OnceCell::const_new();

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

            let fixture = fs::read_to_string("tests/fixtures/schema_cyclic.sql")
                .expect("failed to read tests/fixtures/schema_cyclic.sql");
            sqlx::raw_sql(&fixture)
                .execute(&pool)
                .await
                .expect("failed to apply schema_cyclic.sql");

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

#[tokio::test]
async fn test_resolve_cyclic_schema_produces_valid_plan() {
    let pool = fresh_pool().await;
    let schema = introspect(&pool).await.expect("introspect failed");

    assert_eq!(
        schema.tables.len(),
        2,
        "expected 2 tables in cyclic fixture"
    );
    assert_eq!(
        schema.foreign_keys.len(),
        2,
        "expected 2 FKs in cyclic fixture"
    );

    let plan = resolve(&schema)
        .expect("resolve should succeed by breaking the nullable head_employee_id FK");

    assert_eq!(plan.ordered_tables.len(), 2);
    assert!(plan.ordered_tables.contains(&"departments".to_string()));
    assert!(plan.ordered_tables.contains(&"employees".to_string()));

    let dept_pos = plan
        .ordered_tables
        .iter()
        .position(|t| t == "departments")
        .unwrap();
    let emp_pos = plan
        .ordered_tables
        .iter()
        .position(|t| t == "employees")
        .unwrap();
    assert!(
        dept_pos < emp_pos,
        "departments must precede employees once the head_employee_id FK is deferred; got {:?}",
        plan.ordered_tables
    );

    assert_eq!(
        plan.deferred_updates.len(),
        1,
        "exactly one FK (departments.head_employee_id) should be deferred"
    );
    let upd = &plan.deferred_updates[0];
    assert_eq!(upd.table, "departments");
    assert_eq!(upd.column, "head_employee_id");
    assert_eq!(upd.references_table, "employees");
}
