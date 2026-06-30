use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use sqlx::PgPool;

use crate::lifecycle::{LifecycleConfig, LifecycleEngine, TableLifecycle};
use crate::scenario::{
    load_template, parse_scenario, CountExpression, ScenarioConfig, TableScenario,
};
use crate::{generate as run_generate, GenerateConfig, OutputMode};

#[derive(Debug)]
pub struct Args {
    pub seed: Option<u64>,
    pub rows: usize,
    pub scenario: Option<String>,
    pub file: Option<String>,
    pub entities: Option<HashMap<String, usize>>,
    pub output: Option<String>,
    pub format: String,
    pub fast: bool,
    pub dry_run: bool,
    pub locale: String,
    pub include: Option<Vec<String>>,
    pub exclude: Option<Vec<String>>,
    pub truncate_first: bool,
    pub profile: Option<String>,
    pub scale: f64,
}

pub async fn run(args: Args, url: &str) -> anyhow::Result<()> {
    let output_mode = match (&args.output, args.format.as_str()) {
        (None, _) => OutputMode::DirectInsert,
        (Some(path), "json") => OutputMode::Json(PathBuf::from(path)),
        (Some(path), "sql") => OutputMode::SqlFile(PathBuf::from(path)),
        (Some(path), "copy") => OutputMode::SqlFile(PathBuf::from(path)),
        (Some(_), other) => {
            anyhow::bail!("unknown --format `{other}` (expected sql/json/copy)");
        }
    };

    if args.fast {
        eprintln!("warning: --fast (COPY protocol) not yet implemented; using batch INSERT");
    }
    if args.locale != "en" {
        eprintln!(
            "warning: --locale `{}` not yet implemented; using en",
            args.locale
        );
    }

    let mut scenario = resolve_scenario(args.scenario.as_deref(), args.file.as_deref())?;

    // A production profile supplies the whole scenario (counts + overrides).
    let mut compliance_profile = None;
    if let Some(profile_path) = args.profile.as_deref() {
        if args.scenario.is_some() || args.file.is_some() {
            anyhow::bail!("--profile cannot be combined with --scenario or -f");
        }
        let profile = crate::profile::output::load_profile(std::path::Path::new(profile_path))
            .with_context(|| format!("failed to load profile `{profile_path}`"))?;
        compliance_profile = Some(profile.clone());
        let applicator = crate::profile::ProfileApplicator::new(profile, args.scale)?;
        scenario = Some(applicator.to_scenario()?);
    }

    if let Some(entities) = args.entities {
        scenario = Some(merge_entities(scenario, entities));
    }

    // Precedence: --seed > scenario.seed > time-based.
    let seed = args
        .seed
        .or_else(|| scenario.as_ref().and_then(|s| s.seed))
        .unwrap_or_else(time_seed);

    let config = GenerateConfig {
        seed,
        rows_per_table: args.rows,
        scenario,
        output_mode,
        include_tables: args.include,
        exclude_tables: args.exclude,
        truncate_first: args.truncate_first,
    };

    if args.dry_run {
        // Lifecycle scenarios get a per-bucket plan table.
        if let Some(scenario) = &config.scenario {
            if let Some(lifecycle) = &scenario.lifecycle {
                print_lifecycle_plan(lifecycle, &scenario.table_lifecycles, seed);
                return Ok(());
            }
        }

        println!("Dry run — seed={seed} rows={}", args.rows);
        if let Some(scenario) = &config.scenario {
            println!("  scenario tables: {}", scenario.tables.len());
            for (t, ts) in &scenario.tables {
                println!("    {t}: {:?}", ts.count);
            }
        }
        if let Some(inc) = &config.include_tables {
            println!("  include: {}", inc.join(", "));
        }
        if let Some(exc) = &config.exclude_tables {
            println!("  exclude: {}", exc.join(", "));
        }
        return Ok(());
    }

    let pool = PgPool::connect(url)
        .await
        .context("failed to connect to database")?;
    let result = run_generate(&pool, &config).await?;

    println!(
        "Generated {} rows across {} tables in {:?} (seed: {})",
        result.total_rows,
        result.tables_seeded.len(),
        result.duration,
        result.seed_used,
    );
    for t in &result.tables_seeded {
        println!(
            "  {:<30} {:>6} rows  [{:?}]",
            t.name, t.rows_inserted, t.duration
        );
    }

    // Profile-based generation: report how closely the result matches.
    if let Some(profile) = compliance_profile {
        if matches!(config.output_mode, OutputMode::DirectInsert) {
            match crate::profile::ComplianceValidator::with_default_tolerance(profile)
                .validate(&pool)
                .await
            {
                Ok(report) => report.print(),
                Err(e) => eprintln!("warning: compliance validation skipped: {e}"),
            }
        }
    }

    Ok(())
}

fn resolve_scenario(
    name: Option<&str>,
    file: Option<&str>,
) -> anyhow::Result<Option<ScenarioConfig>> {
    match (name, file) {
        (Some(_), Some(_)) => {
            anyhow::bail!("--scenario and -f are mutually exclusive")
        }
        (Some(name), None) => Ok(Some(load_template(name).map_err(anyhow::Error::from)?)),
        (None, Some(path)) => {
            let yaml = std::fs::read_to_string(path)
                .with_context(|| format!("failed to read scenario file `{path}`"))?;
            Ok(Some(parse_scenario(&yaml).map_err(anyhow::Error::from)?))
        }
        (None, None) => Ok(None),
    }
}

fn print_lifecycle_plan(
    lifecycle: &LifecycleConfig,
    table_lifecycles: &HashMap<String, TableLifecycle>,
    seed: u64,
) {
    let engine = LifecycleEngine::new(lifecycle.clone(), table_lifecycles.clone());
    let report = engine.simulate(seed);

    println!(
        "Lifecycle simulation: {} → {} ({} buckets, seed: {})\n",
        lifecycle.start,
        lifecycle.end,
        report.buckets.len(),
        seed,
    );

    let mut header = format!("  {:<10}", "Bucket");
    for t in &report.table_order {
        header.push_str(&format!(" | {:>22}", truncate(t, 22)));
    }
    println!("{header}");

    for plan in &report.buckets {
        let mut line = format!("  {:<10}", plan.label);
        for stat in &plan.stats {
            let cell = if stat.churned > 0 {
                format!("+{} / -{} / {}", stat.new, stat.churned, stat.active)
            } else {
                format!("+{} / {}", stat.new, stat.active)
            };
            line.push_str(&format!(" | {cell:>22}"));
        }
        println!("{line}");
    }

    println!("\nTotals:");
    for t in &report.table_order {
        let (total, active) = report.totals.get(t).copied().unwrap_or((0, 0));
        let churned = total.saturating_sub(active);
        println!("  {t:<20} {total} created ({active} active, {churned} churned)");
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max.saturating_sub(1)])
    }
}

fn merge_entities(
    base: Option<ScenarioConfig>,
    entities: HashMap<String, usize>,
) -> ScenarioConfig {
    let mut cfg = base.unwrap_or_default();
    for (name, count) in entities {
        cfg.tables
            .entry(name)
            .and_modify(|ts| ts.count = CountExpression::Fixed(count))
            .or_insert(TableScenario {
                count: CountExpression::Fixed(count),
                overrides: HashMap::new(),
            });
    }
    cfg
}

fn time_seed() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(42)
}
