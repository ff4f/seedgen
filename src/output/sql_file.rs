use crate::generators::Value;

const BATCH_SIZE: usize = 1000;

pub fn generate_sql(table_name: &str, columns: &[String], rows: &[Vec<Value>]) -> String {
    let mut out = String::new();
    out.push_str("BEGIN;\n");
    out.push_str("SET session_replication_role = 'replica';\n\n");

    if columns.is_empty() || rows.is_empty() {
        out.push_str(&format!("-- no rows for {table_name}\n\n"));
    } else {
        let col_list = columns
            .iter()
            .map(|c| quote_ident(c))
            .collect::<Vec<_>>()
            .join(", ");

        for batch in rows.chunks(BATCH_SIZE) {
            out.push_str(&format!(
                "INSERT INTO {} ({}) VALUES\n",
                quote_ident(table_name),
                col_list,
            ));
            let lines: Vec<String> = batch
                .iter()
                .map(|row| {
                    let parts: Vec<String> = row.iter().map(format_value).collect();
                    format!("  ({})", parts.join(", "))
                })
                .collect();
            out.push_str(&lines.join(",\n"));
            out.push_str(";\n\n");
        }
    }

    out.push_str("SET session_replication_role = 'origin';\n");
    out.push_str("COMMIT;\n");
    out
}

pub fn format_value(value: &Value) -> String {
    match value {
        Value::Null => "NULL".to_string(),
        Value::Bool(true) => "TRUE".to_string(),
        Value::Bool(false) => "FALSE".to_string(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::String(s) => format!("'{}'", escape_quotes(s)),
        Value::Uuid(s) => format!("'{s}'"),
        Value::Timestamp(dt) => format!("'{}'", dt.format("%Y-%m-%dT%H:%M:%S")),
        Value::Date(d) => format!("'{d}'"),
        Value::Json(j) => format!("'{}'", escape_quotes(&j.to_string())),
    }
}

fn escape_quotes(s: &str) -> String {
    s.replace('\'', "''")
}

pub fn quote_ident(name: &str) -> String {
    let escaped = name.replace('"', "\"\"");
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{NaiveDate, NaiveDateTime};

    fn cols(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_sql_file_wraps_in_transaction() {
        let sql = generate_sql(
            "users",
            &cols(&["email"]),
            &[vec![Value::String("a@b.com".into())]],
        );
        assert!(sql.starts_with("BEGIN;\n"));
        assert!(sql.trim_end().ends_with("COMMIT;"));
    }

    #[test]
    fn test_sql_file_sets_replica_role_then_origin() {
        let sql = generate_sql("t", &cols(&["x"]), &[vec![Value::Int(1)]]);
        let replica_idx = sql.find("session_replication_role = 'replica'").unwrap();
        let origin_idx = sql.find("session_replication_role = 'origin'").unwrap();
        assert!(replica_idx < origin_idx);
        // Replica must come AFTER BEGIN, origin must come BEFORE COMMIT.
        assert!(sql.find("BEGIN;").unwrap() < replica_idx);
        assert!(origin_idx < sql.find("COMMIT;").unwrap());
    }

    #[test]
    fn test_sql_file_produces_valid_insert_statement() {
        let sql = generate_sql(
            "users",
            &cols(&["email", "name"]),
            &[vec![
                Value::String("alice@example.com".into()),
                Value::String("Alice".into()),
            ]],
        );
        assert!(sql.contains("INSERT INTO \"users\" (\"email\", \"name\") VALUES"));
        assert!(sql.contains("('alice@example.com', 'Alice')"));
    }

    #[test]
    fn test_sql_file_escapes_single_quotes() {
        let sql = generate_sql(
            "t",
            &cols(&["msg"]),
            &[vec![Value::String("can't won't".into())]],
        );
        assert!(sql.contains("'can''t won''t'"), "got: {sql}");
    }

    #[test]
    fn test_sql_file_null_unquoted() {
        let sql = generate_sql("t", &cols(&["x"]), &[vec![Value::Null]]);
        assert!(sql.contains("(NULL)"), "got: {sql}");
        // Crucially NOT quoted.
        assert!(!sql.contains("'NULL'"));
    }

    #[test]
    fn test_sql_file_bool_as_true_false() {
        let sql = generate_sql(
            "t",
            &cols(&["a", "b"]),
            &[vec![Value::Bool(true), Value::Bool(false)]],
        );
        assert!(sql.contains("(TRUE, FALSE)"));
    }

    #[test]
    fn test_sql_file_numeric_unquoted() {
        let sql = generate_sql(
            "t",
            &cols(&["a", "b"]),
            &[vec![Value::Int(42), Value::Float(9.99)]],
        );
        assert!(sql.contains("(42, 9.99)"));
        assert!(!sql.contains("'42'"));
        assert!(!sql.contains("'9.99'"));
    }

    #[test]
    fn test_sql_file_timestamp_iso_format() {
        let dt = NaiveDateTime::parse_from_str("2024-12-01 10:30:00", "%Y-%m-%d %H:%M:%S").unwrap();
        let sql = generate_sql("t", &cols(&["when"]), &[vec![Value::Timestamp(dt)]]);
        assert!(sql.contains("'2024-12-01T10:30:00'"), "got: {sql}");
    }

    #[test]
    fn test_sql_file_date_format() {
        let d = NaiveDate::from_ymd_opt(2024, 12, 1).unwrap();
        let sql = generate_sql("t", &cols(&["d"]), &[vec![Value::Date(d)]]);
        assert!(sql.contains("'2024-12-01'"));
    }

    #[test]
    fn test_sql_file_uuid_quoted() {
        let sql = generate_sql(
            "t",
            &cols(&["id"]),
            &[vec![Value::Uuid(
                "550e8400-e29b-41d4-a716-446655440000".into(),
            )]],
        );
        assert!(sql.contains("'550e8400-e29b-41d4-a716-446655440000'"));
    }

    #[test]
    fn test_sql_file_batches_into_multiple_inserts() {
        let rows: Vec<Vec<Value>> = (0..2001).map(|i| vec![Value::Int(i)]).collect();
        let sql = generate_sql("t", &cols(&["x"]), &rows);
        let insert_count = sql.matches("INSERT INTO").count();
        assert_eq!(
            insert_count, 3,
            "2001 rows should produce 3 INSERT statements"
        );
    }

    #[test]
    fn test_sql_file_single_batch_for_under_1000_rows() {
        let rows: Vec<Vec<Value>> = (0..500).map(|i| vec![Value::Int(i)]).collect();
        let sql = generate_sql("t", &cols(&["x"]), &rows);
        assert_eq!(sql.matches("INSERT INTO").count(), 1);
    }

    #[test]
    fn test_sql_file_empty_rows_still_has_wrappers() {
        let sql = generate_sql("t", &cols(&["x"]), &[]);
        assert!(sql.contains("BEGIN;"));
        assert!(sql.contains("COMMIT;"));
        assert!(sql.contains("'replica'"));
        assert!(sql.contains("'origin'"));
        assert!(!sql.contains("INSERT INTO"));
    }

    #[test]
    fn test_sql_file_mixed_row_full_example() {
        let dt = NaiveDateTime::parse_from_str("2024-01-15 09:00:00", "%Y-%m-%d %H:%M:%S").unwrap();
        let sql = generate_sql(
            "events",
            &cols(&["id", "name", "active", "started_at", "bio"]),
            &[vec![
                Value::Int(1),
                Value::String("O'Brien".into()),
                Value::Bool(true),
                Value::Timestamp(dt),
                Value::Null,
            ]],
        );
        assert!(
            sql.contains("(1, 'O''Brien', TRUE, '2024-01-15T09:00:00', NULL)"),
            "got: {sql}"
        );
    }

    #[test]
    fn test_sql_file_quotes_identifiers_with_internal_quotes() {
        let sql = generate_sql("we\"ird", &cols(&["x"]), &[vec![Value::Int(1)]]);
        assert!(sql.contains("\"we\"\"ird\""));
    }
}
