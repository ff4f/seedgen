pub mod queries;
pub mod schema;

use sqlx::PgPool;

pub use schema::{
    Column, Constraint, ConstraintKind, DataType, EnumType, ForeignKey, SchemaGraph, Table,
};

#[derive(Debug, thiserror::Error)]
pub enum IntrospectionError {
    #[error("failed to connect to database: {0}")]
    ConnectionFailed(sqlx::Error),

    #[error("query `{query}` failed: {source}")]
    QueryFailed {
        query: String,
        #[source]
        source: sqlx::Error,
    },
}

pub async fn introspect(pool: &PgPool) -> Result<SchemaGraph, IntrospectionError> {
    let mut tables = queries::query_tables(pool).await?;

    for table in &mut tables {
        table.columns = queries::query_columns(pool, &table.name).await?;
        table.constraints = queries::query_constraints(pool, &table.name).await?;
    }

    let foreign_keys = queries::query_foreign_keys(pool).await?;
    let enums = queries::query_enums(pool).await?;

    Ok(SchemaGraph {
        tables,
        foreign_keys,
        enums,
    })
}
