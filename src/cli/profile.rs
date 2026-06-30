use std::path::Path;

use anyhow::Context;
use sqlx::PgPool;

use crate::introspection::introspect;
use crate::profile::{output, ProfileCollector, ProfileOptions};

#[derive(Debug)]
pub struct Args {
    pub output: String,
    pub format: String,
    pub cardinality_threshold: usize,
    pub exclude: Option<Vec<String>>,
    pub include: Option<Vec<String>>,
    pub strict_security: bool,
    pub no_hourly: bool,
    pub no_monthly: bool,
    pub dry_run_queries: bool,
    pub export_queries: bool,
    pub import_results: Option<String>,
}

pub async fn run(args: Args, url: &str) -> anyhow::Result<()> {
    // Offline import: rebuild a profile from collected results, no DB connection.
    if let Some(results_path) = args.import_results.as_deref() {
        let json = std::fs::read_to_string(results_path)
            .with_context(|| format!("failed to read results `{results_path}`"))?;
        let profile = crate::profile::import_results(&json)?;
        write_profile(&profile, &args.output, &args.format)?;
        println!(
            "Imported profile from {} → {} ({} tables)",
            results_path,
            args.output,
            profile.tables.len(),
        );
        return Ok(());
    }

    let pool = PgPool::connect(url)
        .await
        .context("failed to connect to database")?;
    let schema = introspect(&pool).await?;

    let options = ProfileOptions {
        cardinality_threshold: args.cardinality_threshold,
        exclude_columns: args.exclude.unwrap_or_default(),
        include_columns: args.include.unwrap_or_default(),
        strict_security: args.strict_security,
        capture_hourly: !args.no_hourly,
        capture_monthly: !args.no_monthly,
        ..ProfileOptions::default()
    };

    let mut collector = ProfileCollector::new(&pool, schema, options).await?;

    // Review modes: show the plan without touching the data.
    if args.dry_run_queries {
        let queries = collector.generate_queries();
        println!(
            "The following {} read-only, aggregate-only queries would run:\n",
            queries.len()
        );
        for (i, q) in queries.iter().enumerate() {
            println!("[{}/{}] {}", i + 1, queries.len(), q.sql);
        }
        return Ok(());
    }
    if args.export_queries {
        print!("{}", collector.export_queries());
        return Ok(());
    }

    let profile = collector.collect().await?;
    write_profile(&profile, &args.output, &args.format)?;

    let audit_path = Path::new(".seedgen-profile-audit.log");
    collector.write_audit_log(audit_path)?;

    println!(
        "Profiled {} tables → {} ({} queries run, {} sensitive columns skipped)",
        profile.tables.len(),
        args.output,
        collector.audit_log().query_count(),
        profile.options.skipped_sensitive.len(),
    );
    println!("Audit log: {}", audit_path.display());
    Ok(())
}

fn write_profile(
    profile: &crate::profile::DatabaseProfile,
    output_path: &str,
    format: &str,
) -> anyhow::Result<()> {
    let serialized = match format {
        "json" => output::to_json(profile)?,
        "yaml" => output::to_yaml(profile)?,
        other => anyhow::bail!("unknown --format `{other}` (expected yaml or json)"),
    };
    std::fs::write(output_path, serialized)
        .with_context(|| format!("failed to write `{output_path}`"))?;
    Ok(())
}
