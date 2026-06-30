use std::collections::HashMap;

use clap::{Parser, Subcommand};
use clap_complete::Shell;

#[derive(Parser, Debug)]
#[command(name = "seedgen")]
#[command(about = "Zero-config database seed data generator", long_about = None)]
#[command(version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    /// PostgreSQL connection URL.
    #[arg(long, short = 'u', env = "DATABASE_URL", global = true)]
    pub url: Option<String>,

    /// Increase verbosity (-v, -vv, -vvv).
    #[arg(long, short, action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,

    /// Suppress output except errors.
    #[arg(long, short, global = true)]
    pub quiet: bool,

    /// Disable colored output.
    #[arg(long, global = true)]
    pub no_color: bool,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Introspect database schema and print it.
    Introspect {
        /// Output format: `table`, `json`, or `yaml`.
        #[arg(long, default_value = "table")]
        format: String,

        /// Write to this file instead of stdout.
        #[arg(long)]
        output: Option<String>,

        /// Comma-separated list of tables to include.
        #[arg(long, value_delimiter = ',')]
        include: Option<Vec<String>>,

        /// Comma-separated list of tables to exclude.
        #[arg(long, value_delimiter = ',')]
        exclude: Option<Vec<String>>,
    },

    /// Generate and insert seed data.
    Generate {
        /// Seed for deterministic output. Defaults to time-based.
        #[arg(long)]
        seed: Option<u64>,

        /// Default rows per table.
        #[arg(long, default_value = "10")]
        rows: usize,

        /// Built-in scenario name: ecommerce, saas, blog, social.
        #[arg(long)]
        scenario: Option<String>,

        /// Custom scenario YAML file.
        #[arg(long, short = 'f')]
        file: Option<String>,

        /// Per-table row counts, e.g. `users=100,orders=500`.
        #[arg(long, value_parser = parse_entities)]
        entities: Option<HashMap<String, usize>>,

        /// Write generated SQL/JSON to this file instead of inserting.
        #[arg(long)]
        output: Option<String>,

        /// Output format when --output is set: `sql`, `json`, `copy`.
        #[arg(long, default_value = "sql")]
        format: String,

        /// Use the COPY protocol for faster inserts.
        #[arg(long)]
        fast: bool,

        /// Print the generation plan without inserting.
        #[arg(long)]
        dry_run: bool,

        /// Locale for fake data: en, id, ja, de, fr.
        #[arg(long, default_value = "en")]
        locale: String,

        /// Comma-separated list of tables to include.
        #[arg(long, value_delimiter = ',')]
        include: Option<Vec<String>>,

        /// Comma-separated list of tables to exclude.
        #[arg(long, value_delimiter = ',')]
        exclude: Option<Vec<String>>,

        /// TRUNCATE all target tables before inserting.
        #[arg(long)]
        truncate_first: bool,

        /// Generate from a production profile file (statistics-driven).
        #[arg(long)]
        profile: Option<String>,

        /// Scale factor for `--profile` generation (0 < scale <= 1).
        #[arg(long, default_value = "1.0")]
        scale: f64,
    },

    /// Profile a database's statistics (read-only, aggregate-only).
    Profile {
        /// Write the profile to this file.
        #[arg(long, default_value = "prod-profile.yaml")]
        output: String,

        /// Profile format: `yaml` or `json`.
        #[arg(long, default_value = "yaml")]
        format: String,

        /// Max distinct values before a column is treated as high-cardinality.
        #[arg(long, default_value = "50")]
        cardinality_threshold: usize,

        /// Columns to exclude (`table.column`), comma-separated.
        #[arg(long, value_delimiter = ',')]
        exclude: Option<Vec<String>>,

        /// Columns to force-include even if sensitive (`table.column`).
        #[arg(long, value_delimiter = ',')]
        include: Option<Vec<String>>,

        /// Refuse to profile when connected as a superuser.
        #[arg(long)]
        strict_security: bool,

        /// Skip hourly-density capture for timestamp columns.
        #[arg(long)]
        no_hourly: bool,

        /// Skip monthly-density capture for timestamp columns.
        #[arg(long)]
        no_monthly: bool,

        /// Print the queries that would run, without executing them.
        #[arg(long)]
        dry_run_queries: bool,

        /// Print the queries as a runnable SQL file, without executing them.
        #[arg(long)]
        export_queries: bool,

        /// Build the profile offline from an externally-collected results file
        /// (no database connection needed).
        #[arg(long)]
        import_results: Option<String>,
    },

    /// TRUNCATE tables in safe order.
    Reset {
        /// Required safety flag.
        #[arg(long)]
        confirm: bool,

        /// Only truncate these tables (comma-separated).
        #[arg(long, value_delimiter = ',')]
        only: Option<Vec<String>>,

        /// Use TRUNCATE CASCADE.
        #[arg(long, default_value = "true")]
        cascade: bool,
    },

    /// Validate a scenario YAML file against the schema.
    Validate {
        /// Scenario YAML file to validate.
        #[arg(long, short = 'f')]
        file: String,
    },

    /// Start the MCP server.
    McpServer {
        /// Transport: stdio or http.
        #[arg(long, default_value = "stdio")]
        transport: String,

        /// Port for the HTTP transport.
        #[arg(long, default_value = "3100")]
        port: u16,
    },

    /// Print shell completion script.
    Completions {
        /// Shell to generate completions for.
        shell: Shell,
    },
}

fn parse_entities(s: &str) -> Result<HashMap<String, usize>, String> {
    let mut map = HashMap::new();
    for pair in s.split(',') {
        let (k, v) = pair
            .split_once('=')
            .ok_or_else(|| format!("expected `name=count`, got `{pair}`"))?;
        let k = k.trim();
        let count: usize = v
            .trim()
            .parse()
            .map_err(|e| format!("invalid count for `{k}`: {e}"))?;
        if k.is_empty() {
            return Err("empty table name".into());
        }
        map.insert(k.to_string(), count);
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn test_cli_builds_without_panic() {
        Cli::command().debug_assert();
    }

    #[test]
    fn test_parse_entities_simple() {
        let m = parse_entities("users=100,orders=500").unwrap();
        assert_eq!(m.get("users"), Some(&100));
        assert_eq!(m.get("orders"), Some(&500));
    }

    #[test]
    fn test_parse_entities_with_whitespace() {
        let m = parse_entities("users = 50, products = 200").unwrap();
        assert_eq!(m.get("users"), Some(&50));
        assert_eq!(m.get("products"), Some(&200));
    }

    #[test]
    fn test_parse_entities_missing_equals_errors() {
        assert!(parse_entities("users 100").is_err());
    }

    #[test]
    fn test_parse_entities_invalid_count_errors() {
        assert!(parse_entities("users=abc").is_err());
    }

    #[test]
    fn test_parse_entities_empty_name_errors() {
        assert!(parse_entities("=100").is_err());
    }
}
