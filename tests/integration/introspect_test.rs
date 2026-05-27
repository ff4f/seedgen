#![cfg(feature = "integration")]

use std::env;
use std::fs;

use seedgen::introspection::{introspect, ConstraintKind, DataType};
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

#[tokio::test]
async fn test_introspect_finds_three_tables() {
    let pool = fresh_pool().await;
    let schema = introspect(&pool).await.expect("introspect failed");

    assert_eq!(schema.tables.len(), 3, "expected exactly 3 tables");
    let names: Vec<&str> = schema.tables.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"users"), "missing users: {:?}", names);
    assert!(names.contains(&"posts"), "missing posts: {:?}", names);
    assert!(names.contains(&"comments"), "missing comments: {:?}", names);
}

#[tokio::test]
async fn test_introspect_finds_three_foreign_keys() {
    let pool = fresh_pool().await;
    let schema = introspect(&pool).await.expect("introspect failed");

    let has_fk = |from_t: &str, from_c: &str, to_t: &str, to_c: &str| {
        schema.foreign_keys.iter().any(|fk| {
            fk.from_table == from_t
                && fk.from_column == from_c
                && fk.to_table == to_t
                && fk.to_column == to_c
        })
    };

    assert!(
        has_fk("posts", "user_id", "users", "id"),
        "missing posts.user_id -> users.id (have {:?})",
        schema.foreign_keys
    );
    assert!(
        has_fk("comments", "post_id", "posts", "id"),
        "missing comments.post_id -> posts.id"
    );
    assert!(
        has_fk("comments", "user_id", "users", "id"),
        "missing comments.user_id -> users.id"
    );
    assert_eq!(schema.foreign_keys.len(), 3, "expected exactly 3 FKs");
}

#[tokio::test]
async fn test_introspect_detects_unique_constraints() {
    let pool = fresh_pool().await;
    let schema = introspect(&pool).await.expect("introspect failed");

    let users = schema.table("users").expect("users table missing");
    let user_unique: Vec<&Vec<String>> = users
        .constraints
        .iter()
        .filter(|c| c.kind == ConstraintKind::Unique)
        .map(|c| &c.columns)
        .collect();
    assert!(
        user_unique
            .iter()
            .any(|cols| cols == &&vec!["email".to_string()]),
        "users.email should have a UNIQUE constraint; have {:?}",
        user_unique
    );

    let posts = schema.table("posts").expect("posts table missing");
    let post_unique: Vec<&Vec<String>> = posts
        .constraints
        .iter()
        .filter(|c| c.kind == ConstraintKind::Unique)
        .map(|c| &c.columns)
        .collect();
    assert!(
        post_unique
            .iter()
            .any(|cols| cols == &&vec!["slug".to_string()]),
        "posts.slug should have a UNIQUE constraint; have {:?}",
        post_unique
    );
}

#[tokio::test]
async fn test_introspect_detects_not_null() {
    let pool = fresh_pool().await;
    let schema = introspect(&pool).await.expect("introspect failed");

    let users = schema.table("users").expect("users missing");
    assert!(!users.column("id").unwrap().is_nullable);
    assert!(!users.column("email").unwrap().is_nullable);
    assert!(!users.column("name").unwrap().is_nullable);
    assert!(users.column("bio").unwrap().is_nullable);

    let posts = schema.table("posts").expect("posts missing");
    assert!(!posts.column("id").unwrap().is_nullable);
    assert!(!posts.column("user_id").unwrap().is_nullable);
    assert!(!posts.column("title").unwrap().is_nullable);
    assert!(!posts.column("slug").unwrap().is_nullable);
    assert!(posts.column("body").unwrap().is_nullable);
    assert!(posts.column("published_at").unwrap().is_nullable);

    let comments = schema.table("comments").expect("comments missing");
    assert!(!comments.column("post_id").unwrap().is_nullable);
    assert!(!comments.column("user_id").unwrap().is_nullable);
    assert!(!comments.column("content").unwrap().is_nullable);
}

#[tokio::test]
async fn test_introspect_detects_data_types() {
    let pool = fresh_pool().await;
    let schema = introspect(&pool).await.expect("introspect failed");

    let users = schema.table("users").expect("users missing");
    assert_eq!(users.column("id").unwrap().data_type, DataType::Integer);
    assert_eq!(users.column("email").unwrap().data_type, DataType::Varchar);
    assert_eq!(users.column("name").unwrap().data_type, DataType::Varchar);
    assert_eq!(users.column("bio").unwrap().data_type, DataType::Text);
    assert_eq!(
        users.column("is_active").unwrap().data_type,
        DataType::Boolean
    );
    assert_eq!(
        users.column("created_at").unwrap().data_type,
        DataType::Timestamp
    );

    let posts = schema.table("posts").expect("posts missing");
    assert_eq!(
        posts.column("user_id").unwrap().data_type,
        DataType::Integer
    );
    assert_eq!(posts.column("title").unwrap().data_type, DataType::Varchar);
    assert_eq!(posts.column("title").unwrap().max_length, Some(200));
    assert_eq!(posts.column("slug").unwrap().max_length, Some(200));
    assert_eq!(posts.column("body").unwrap().data_type, DataType::Text);
    assert_eq!(
        posts.column("published_at").unwrap().data_type,
        DataType::Timestamp
    );

    let id = users.column("id").unwrap();
    assert!(
        id.is_identity || id.default_value.is_some(),
        "SERIAL id should be identity or have a sequence default"
    );
}
