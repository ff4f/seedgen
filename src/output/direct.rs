use sqlx::postgres::PgArguments;
use sqlx::{PgPool, Postgres, Row, Transaction};

use crate::generators::Value;

const BATCH_SIZE: usize = 1000;

#[derive(Debug, thiserror::Error)]
pub enum OutputError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("row {row_index} has {got} columns, expected {expected}")]
    ShapeMismatch {
        row_index: usize,
        expected: usize,
        got: usize,
    },

    #[error("no columns provided for insert into `{0}`")]
    NoColumns(String),
}

pub async fn insert_rows(
    pool: &PgPool,
    table_name: &str,
    columns: &[String],
    rows: &[Vec<Value>],
) -> Result<Vec<i64>, OutputError> {
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    if columns.is_empty() {
        return Err(OutputError::NoColumns(table_name.to_string()));
    }
    for (i, row) in rows.iter().enumerate() {
        if row.len() != columns.len() {
            return Err(OutputError::ShapeMismatch {
                row_index: i,
                expected: columns.len(),
                got: row.len(),
            });
        }
    }

    let mut tx = pool.begin().await?;
    let mut all_ids = Vec::with_capacity(rows.len());

    for batch in rows.chunks(BATCH_SIZE) {
        let ids = insert_batch(&mut tx, table_name, columns, batch).await?;
        all_ids.extend(ids);
    }

    tx.commit().await?;
    Ok(all_ids)
}

async fn insert_batch(
    tx: &mut Transaction<'_, Postgres>,
    table_name: &str,
    columns: &[String],
    batch: &[Vec<Value>],
) -> Result<Vec<i64>, OutputError> {
    let col_count = columns.len();
    let col_list = columns
        .iter()
        .map(|c| quote_ident(c))
        .collect::<Vec<_>>()
        .join(", ");

    let placeholders = (0..batch.len())
        .map(|row_idx| {
            let parts = (0..col_count)
                .map(|c| format!("${}", row_idx * col_count + c + 1))
                .collect::<Vec<_>>()
                .join(", ");
            format!("({parts})")
        })
        .collect::<Vec<_>>()
        .join(", ");

    let sql = format!(
        "INSERT INTO {} ({}) VALUES {} RETURNING id",
        quote_ident(table_name),
        col_list,
        placeholders,
    );

    let mut query = sqlx::query(&sql);
    for row in batch {
        for value in row {
            query = bind_value(query, value);
        }
    }

    let pg_rows = query.fetch_all(&mut **tx).await?;
    let mut ids = Vec::with_capacity(pg_rows.len());
    for r in pg_rows {
        // SERIAL (int4) and BIGSERIAL (int8) are both common; try widening from i32 first.
        let id = match r.try_get::<i32, _>("id") {
            Ok(v) => v as i64,
            Err(_) => r.try_get::<i64, _>("id")?,
        };
        ids.push(id);
    }
    Ok(ids)
}

pub async fn truncate_tables(pool: &PgPool, tables: &[String]) -> Result<(), OutputError> {
    if tables.is_empty() {
        return Ok(());
    }
    let quoted = tables
        .iter()
        .map(|t| quote_ident(t))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!("TRUNCATE TABLE {quoted} RESTART IDENTITY CASCADE");
    sqlx::raw_sql(&sql).execute(pool).await?;
    Ok(())
}

fn quote_ident(name: &str) -> String {
    let escaped = name.replace('"', "\"\"");
    format!("\"{escaped}\"")
}

fn bind_value<'q>(
    q: sqlx::query::Query<'q, Postgres, PgArguments>,
    value: &Value,
) -> sqlx::query::Query<'q, Postgres, PgArguments> {
    match value {
        Value::String(s) => q.bind(s.clone()),
        Value::Int(i) => q.bind(*i),
        Value::Float(f) => q.bind(*f),
        Value::Bool(b) => q.bind(*b),
        Value::Null => q.bind(Option::<String>::None),
        Value::Uuid(s) => q.bind(sqlx::types::Uuid::parse_str(s).expect("valid uuid")),
        Value::Timestamp(dt) => q.bind(*dt),
        Value::Date(d) => q.bind(*d),
        Value::Json(j) => q.bind(j.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quote_ident_simple() {
        assert_eq!(quote_ident("users"), "\"users\"");
    }

    #[test]
    fn test_quote_ident_escapes_internal_quotes() {
        assert_eq!(quote_ident("we\"ird"), "\"we\"\"ird\"");
    }
}
