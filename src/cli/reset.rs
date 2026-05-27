use anyhow::Context;
use sqlx::PgPool;

use crate::introspection::introspect;
use crate::output::truncate_tables;
use crate::resolver::resolve;

#[derive(Debug)]
pub struct Args {
    pub confirm: bool,
    pub only: Option<Vec<String>>,
    pub cascade: bool,
}

pub async fn run(args: Args, url: &str) -> anyhow::Result<()> {
    if !args.confirm {
        anyhow::bail!(
            "reset is destructive — pass --confirm to proceed (this TRUNCATEs target tables)"
        );
    }

    if url.to_ascii_lowercase().contains("prod") {
        anyhow::bail!(
            "refusing to reset a database whose URL contains `prod`. \
             override is intentional and not yet implemented."
        );
    }

    let pool = PgPool::connect(url)
        .await
        .context("failed to connect to database")?;

    let tables = match args.only {
        Some(list) if !list.is_empty() => list,
        _ => {
            let schema = introspect(&pool).await?;
            let plan = resolve(&schema)?;
            let mut all = plan.ordered_tables;
            // Children first for safer truncation (CASCADE handles the rest anyway).
            all.reverse();
            all
        }
    };

    if tables.is_empty() {
        println!("No tables to reset.");
        return Ok(());
    }

    truncate_tables(&pool, &tables).await?;
    println!(
        "Truncated {} table{}{}",
        tables.len(),
        if tables.len() == 1 { "" } else { "s" },
        if args.cascade { " (CASCADE)" } else { "" },
    );
    Ok(())
}
