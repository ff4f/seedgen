pub mod direct;
pub mod sql_file;

pub use direct::{insert_rows, truncate_tables, OutputError};
pub use sql_file::generate_sql;
