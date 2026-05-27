use sqlx::{PgPool, Row};

use super::schema::{Column, Constraint, ConstraintKind, DataType, EnumType, ForeignKey, Table};
use super::IntrospectionError;

pub type Result<T> = std::result::Result<T, IntrospectionError>;

fn fail(query: &'static str) -> impl Fn(sqlx::Error) -> IntrospectionError {
    move |source| IntrospectionError::QueryFailed {
        query: query.to_string(),
        source,
    }
}

const TABLES_SQL: &str = r#"
    SELECT table_name
    FROM information_schema.tables
    WHERE table_schema = 'public'
      AND table_type = 'BASE TABLE'
    ORDER BY table_name
"#;

pub async fn query_tables(pool: &PgPool) -> Result<Vec<Table>> {
    let err = fail("query_tables");
    let rows = sqlx::query(TABLES_SQL)
        .fetch_all(pool)
        .await
        .map_err(&err)?;
    let mut tables = Vec::with_capacity(rows.len());
    for row in rows {
        let name: String = row.try_get("table_name").map_err(&err)?;
        tables.push(Table {
            name,
            columns: Vec::new(),
            constraints: Vec::new(),
        });
    }
    Ok(tables)
}

const COLUMNS_SQL: &str = r#"
    SELECT
        column_name,
        data_type,
        udt_name,
        is_nullable,
        is_identity,
        column_default,
        character_maximum_length,
        numeric_precision,
        numeric_scale
    FROM information_schema.columns
    WHERE table_schema = 'public'
      AND table_name = $1
    ORDER BY ordinal_position
"#;

pub async fn query_columns(pool: &PgPool, table_name: &str) -> Result<Vec<Column>> {
    let err = fail("query_columns");
    let rows = sqlx::query(COLUMNS_SQL)
        .bind(table_name)
        .fetch_all(pool)
        .await
        .map_err(&err)?;

    let generated = query_generated_columns(pool, table_name).await?;

    let mut columns = Vec::with_capacity(rows.len());
    for row in rows {
        let name: String = row.try_get("column_name").map_err(&err)?;
        let data_type_str: String = row.try_get("data_type").map_err(&err)?;
        let udt_name: String = row.try_get("udt_name").map_err(&err)?;
        let is_nullable: String = row.try_get("is_nullable").map_err(&err)?;
        let is_identity: String = row.try_get("is_identity").map_err(&err)?;
        let default_value: Option<String> = row.try_get("column_default").map_err(&err)?;
        let max_length: Option<i32> = row.try_get("character_maximum_length").map_err(&err)?;
        let numeric_precision: Option<i32> = row.try_get("numeric_precision").map_err(&err)?;
        let numeric_scale: Option<i32> = row.try_get("numeric_scale").map_err(&err)?;

        let is_generated = generated.iter().any(|g| g == &name);

        columns.push(Column {
            data_type: map_data_type(&data_type_str, &udt_name),
            is_nullable: yes_no(&is_nullable),
            is_identity: yes_no(&is_identity),
            is_generated,
            default_value,
            max_length: max_length.map(|v| v as u32),
            numeric_precision: numeric_precision.map(|v| v as u32),
            numeric_scale: numeric_scale.map(|v| v as u32),
            name,
        });
    }
    Ok(columns)
}

const FOREIGN_KEYS_SQL: &str = r#"
    SELECT
        tc.table_name           AS from_table,
        kcu.column_name         AS from_column,
        ccu.table_name          AS to_table,
        ccu.column_name         AS to_column,
        col.is_nullable         AS is_nullable,
        tc.is_deferrable        AS is_deferrable,
        kcu.ordinal_position    AS ord
    FROM information_schema.table_constraints tc
    JOIN information_schema.key_column_usage kcu
      ON tc.constraint_name   = kcu.constraint_name
     AND tc.constraint_schema = kcu.constraint_schema
    JOIN information_schema.referential_constraints rc
      ON tc.constraint_name   = rc.constraint_name
     AND tc.constraint_schema = rc.constraint_schema
    JOIN information_schema.constraint_column_usage ccu
      ON rc.unique_constraint_name   = ccu.constraint_name
     AND rc.unique_constraint_schema = ccu.constraint_schema
    JOIN information_schema.columns col
      ON col.table_schema = tc.table_schema
     AND col.table_name   = tc.table_name
     AND col.column_name  = kcu.column_name
    WHERE tc.constraint_type = 'FOREIGN KEY'
      AND tc.table_schema    = 'public'
    ORDER BY tc.table_name, tc.constraint_name, kcu.ordinal_position
"#;

pub async fn query_foreign_keys(pool: &PgPool) -> Result<Vec<ForeignKey>> {
    let err = fail("query_foreign_keys");
    let rows = sqlx::query(FOREIGN_KEYS_SQL)
        .fetch_all(pool)
        .await
        .map_err(&err)?;
    let mut fks = Vec::with_capacity(rows.len());
    for row in rows {
        let from_table: String = row.try_get("from_table").map_err(&err)?;
        let from_column: String = row.try_get("from_column").map_err(&err)?;
        let to_table: String = row.try_get("to_table").map_err(&err)?;
        let to_column: String = row.try_get("to_column").map_err(&err)?;
        let is_nullable: String = row.try_get("is_nullable").map_err(&err)?;
        let is_deferrable: String = row.try_get("is_deferrable").map_err(&err)?;
        fks.push(ForeignKey {
            from_table,
            from_column,
            to_table,
            to_column,
            is_nullable: yes_no(&is_nullable),
            is_deferrable: yes_no(&is_deferrable),
        });
    }
    Ok(fks)
}

const UNIQUE_CONSTRAINTS_SQL: &str = r#"
    SELECT
        tc.constraint_name,
        kcu.column_name,
        kcu.ordinal_position
    FROM information_schema.table_constraints tc
    JOIN information_schema.key_column_usage kcu
      ON tc.constraint_name   = kcu.constraint_name
     AND tc.constraint_schema = kcu.constraint_schema
    WHERE tc.constraint_type = 'UNIQUE'
      AND tc.table_schema    = 'public'
      AND tc.table_name      = $1
    ORDER BY tc.constraint_name, kcu.ordinal_position
"#;

const CHECK_CONSTRAINTS_SQL: &str = r#"
    SELECT
        tc.constraint_name,
        cc.check_clause,
        ccu.column_name
    FROM information_schema.table_constraints tc
    JOIN information_schema.check_constraints cc
      ON tc.constraint_name   = cc.constraint_name
     AND tc.constraint_schema = cc.constraint_schema
    LEFT JOIN information_schema.constraint_column_usage ccu
      ON tc.constraint_name   = ccu.constraint_name
     AND tc.constraint_schema = ccu.constraint_schema
    WHERE tc.constraint_type = 'CHECK'
      AND tc.table_schema    = 'public'
      AND tc.table_name      = $1
      AND cc.check_clause NOT LIKE '%IS NOT NULL%'
    ORDER BY tc.constraint_name
"#;

pub async fn query_constraints(pool: &PgPool, table_name: &str) -> Result<Vec<Constraint>> {
    let mut constraints: Vec<Constraint> = Vec::new();

    let unique_err = fail("query_constraints/unique");
    let unique_rows = sqlx::query(UNIQUE_CONSTRAINTS_SQL)
        .bind(table_name)
        .fetch_all(pool)
        .await
        .map_err(&unique_err)?;

    let mut current_name: Option<String> = None;
    for row in unique_rows {
        let name: String = row.try_get("constraint_name").map_err(&unique_err)?;
        let column: String = row.try_get("column_name").map_err(&unique_err)?;
        if current_name.as_deref() == Some(name.as_str()) {
            if let Some(last) = constraints.last_mut() {
                last.columns.push(column);
            }
        } else {
            constraints.push(Constraint {
                kind: ConstraintKind::Unique,
                columns: vec![column],
                check_expression: None,
            });
            current_name = Some(name);
        }
    }

    let check_err = fail("query_constraints/check");
    let check_rows = sqlx::query(CHECK_CONSTRAINTS_SQL)
        .bind(table_name)
        .fetch_all(pool)
        .await
        .map_err(&check_err)?;

    let mut current_check: Option<String> = None;
    for row in check_rows {
        let name: String = row.try_get("constraint_name").map_err(&check_err)?;
        let clause: String = row.try_get("check_clause").map_err(&check_err)?;
        let column: Option<String> = row.try_get("column_name").map_err(&check_err)?;
        if current_check.as_deref() == Some(name.as_str()) {
            if let (Some(last), Some(col)) = (constraints.last_mut(), column) {
                if !last.columns.contains(&col) {
                    last.columns.push(col);
                }
            }
        } else {
            constraints.push(Constraint {
                kind: ConstraintKind::Check,
                columns: column.map(|c| vec![c]).unwrap_or_default(),
                check_expression: Some(clause),
            });
            current_check = Some(name);
        }
    }

    Ok(constraints)
}

const ENUMS_SQL: &str = r#"
    SELECT
        t.typname           AS name,
        e.enumlabel         AS value
    FROM pg_type t
    JOIN pg_enum e ON t.oid = e.enumtypid
    JOIN pg_namespace n ON t.typnamespace = n.oid
    WHERE n.nspname = 'public'
    ORDER BY t.typname, e.enumsortorder
"#;

pub async fn query_enums(pool: &PgPool) -> Result<Vec<EnumType>> {
    let err = fail("query_enums");
    let rows = sqlx::query(ENUMS_SQL).fetch_all(pool).await.map_err(&err)?;
    let mut enums: Vec<EnumType> = Vec::new();
    for row in rows {
        let name: String = row.try_get("name").map_err(&err)?;
        let value: String = row.try_get("value").map_err(&err)?;
        match enums.last_mut() {
            Some(last) if last.name == name => last.values.push(value),
            _ => enums.push(EnumType {
                name,
                values: vec![value],
            }),
        }
    }
    Ok(enums)
}

const GENERATED_COLUMNS_SQL: &str = r#"
    SELECT a.attname AS name
    FROM pg_attribute a
    JOIN pg_class c     ON a.attrelid    = c.oid
    JOIN pg_namespace n ON c.relnamespace = n.oid
    WHERE n.nspname     = 'public'
      AND c.relname     = $1
      AND a.attgenerated <> ''
      AND a.attnum      > 0
      AND NOT a.attisdropped
"#;

pub async fn query_generated_columns(pool: &PgPool, table_name: &str) -> Result<Vec<String>> {
    let err = fail("query_generated_columns");
    let rows = sqlx::query(GENERATED_COLUMNS_SQL)
        .bind(table_name)
        .fetch_all(pool)
        .await
        .map_err(&err)?;
    let mut names = Vec::with_capacity(rows.len());
    for row in rows {
        names.push(row.try_get::<String, _>("name").map_err(&err)?);
    }
    Ok(names)
}

fn yes_no(s: &str) -> bool {
    s.eq_ignore_ascii_case("YES")
}

fn map_data_type(data_type: &str, udt_name: &str) -> DataType {
    match data_type {
        "ARRAY" => {
            let inner = udt_name.strip_prefix('_').unwrap_or(udt_name);
            DataType::Array(Box::new(map_udt(inner)))
        }
        "USER-DEFINED" => DataType::Enum(udt_name.to_string()),
        _ => map_udt(udt_name),
    }
}

fn map_udt(udt: &str) -> DataType {
    match udt {
        "bool" => DataType::Boolean,
        "int2" => DataType::SmallInt,
        "int4" => DataType::Integer,
        "int8" => DataType::BigInt,
        "float4" => DataType::Real,
        "float8" => DataType::DoublePrecision,
        "numeric" => DataType::Numeric,
        "bpchar" => DataType::Char,
        "varchar" => DataType::Varchar,
        "text" => DataType::Text,
        "bytea" => DataType::Bytea,
        "date" => DataType::Date,
        "time" => DataType::Time,
        "timetz" => DataType::TimeTz,
        "timestamp" => DataType::Timestamp,
        "timestamptz" => DataType::TimestampTz,
        "interval" => DataType::Interval,
        "uuid" => DataType::Uuid,
        "json" => DataType::Json,
        "jsonb" => DataType::Jsonb,
        "inet" => DataType::Inet,
        "cidr" => DataType::Cidr,
        "macaddr" => DataType::MacAddr,
        "money" => DataType::Money,
        other => DataType::Other(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_map_udt_known_scalars() {
        assert_eq!(map_udt("bool"), DataType::Boolean);
        assert_eq!(map_udt("int4"), DataType::Integer);
        assert_eq!(map_udt("int8"), DataType::BigInt);
        assert_eq!(map_udt("varchar"), DataType::Varchar);
        assert_eq!(map_udt("timestamptz"), DataType::TimestampTz);
        assert_eq!(map_udt("jsonb"), DataType::Jsonb);
        assert_eq!(map_udt("uuid"), DataType::Uuid);
    }

    #[test]
    fn test_map_udt_unknown_falls_back_to_other() {
        assert_eq!(map_udt("tsvector"), DataType::Other("tsvector".into()));
    }

    #[test]
    fn test_map_data_type_array() {
        assert_eq!(
            map_data_type("ARRAY", "_int4"),
            DataType::Array(Box::new(DataType::Integer))
        );
        assert_eq!(
            map_data_type("ARRAY", "_text"),
            DataType::Array(Box::new(DataType::Text))
        );
    }

    #[test]
    fn test_map_data_type_user_defined_is_enum() {
        assert_eq!(
            map_data_type("USER-DEFINED", "order_status"),
            DataType::Enum("order_status".into())
        );
    }

    #[test]
    fn test_map_data_type_scalar_uses_udt() {
        assert_eq!(map_data_type("integer", "int4"), DataType::Integer);
        assert_eq!(
            map_data_type("character varying", "varchar"),
            DataType::Varchar
        );
    }

    #[test]
    fn test_yes_no_parsing() {
        assert!(yes_no("YES"));
        assert!(yes_no("yes"));
        assert!(!yes_no("NO"));
        assert!(!yes_no(""));
    }
}
