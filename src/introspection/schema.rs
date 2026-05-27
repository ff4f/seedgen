use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SchemaGraph {
    pub tables: Vec<Table>,
    pub foreign_keys: Vec<ForeignKey>,
    pub enums: Vec<EnumType>,
}

impl SchemaGraph {
    pub fn table(&self, name: &str) -> Option<&Table> {
        self.tables.iter().find(|t| t.name == name)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Table {
    pub name: String,
    pub columns: Vec<Column>,
    pub constraints: Vec<Constraint>,
}

impl Table {
    pub fn column(&self, name: &str) -> Option<&Column> {
        self.columns.iter().find(|c| c.name == name)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Column {
    pub name: String,
    pub data_type: DataType,
    pub is_nullable: bool,
    pub is_identity: bool,
    pub is_generated: bool,
    pub default_value: Option<String>,
    pub max_length: Option<u32>,
    pub numeric_precision: Option<u32>,
    pub numeric_scale: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForeignKey {
    pub from_table: String,
    pub from_column: String,
    pub to_table: String,
    pub to_column: String,
    pub is_nullable: bool,
    pub is_deferrable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Constraint {
    pub kind: ConstraintKind,
    pub columns: Vec<String>,
    pub check_expression: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ConstraintKind {
    PrimaryKey,
    Unique,
    Check,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnumType {
    pub name: String,
    pub values: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DataType {
    Boolean,

    SmallInt,
    Integer,
    BigInt,

    Real,
    DoublePrecision,
    Numeric,

    Char,
    Varchar,
    Text,

    Bytea,

    Date,
    Time,
    TimeTz,
    Timestamp,
    TimestampTz,
    Interval,

    Uuid,
    Json,
    Jsonb,

    Inet,
    Cidr,
    MacAddr,
    Money,

    Array(Box<DataType>),
    Enum(String),
    Other(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_table(name: &str, cols: &[&str]) -> Table {
        Table {
            name: name.to_string(),
            columns: cols
                .iter()
                .map(|c| Column {
                    name: c.to_string(),
                    data_type: DataType::Text,
                    is_nullable: true,
                    is_identity: false,
                    is_generated: false,
                    default_value: None,
                    max_length: None,
                    numeric_precision: None,
                    numeric_scale: None,
                })
                .collect(),
            constraints: vec![],
        }
    }

    #[test]
    fn test_schema_graph_table_lookup_hit() {
        let graph = SchemaGraph {
            tables: vec![
                sample_table("users", &["id"]),
                sample_table("orders", &["id"]),
            ],
            ..Default::default()
        };
        assert_eq!(graph.table("orders").unwrap().name, "orders");
    }

    #[test]
    fn test_schema_graph_table_lookup_miss() {
        let graph = SchemaGraph::default();
        assert!(graph.table("missing").is_none());
    }

    #[test]
    fn test_table_column_lookup_hit() {
        let table = sample_table("users", &["id", "email"]);
        assert_eq!(table.column("email").unwrap().name, "email");
    }

    #[test]
    fn test_table_column_lookup_miss() {
        let table = sample_table("users", &["id"]);
        assert!(table.column("missing").is_none());
    }
}
