pub mod commands;
pub mod generate;
pub mod introspect;
pub mod reset;

use clap::{CommandFactory, Parser};

pub use commands::{Cli, Commands};

pub async fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    dispatch(cli).await
}

async fn dispatch(cli: Cli) -> anyhow::Result<()> {
    match cli.command {
        Commands::Introspect {
            format,
            output,
            include,
            exclude,
        } => {
            let url = require_url(cli.url)?;
            introspect::run(
                introspect::Args {
                    format,
                    output,
                    include,
                    exclude,
                },
                &url,
            )
            .await
        }
        Commands::Generate {
            seed,
            rows,
            scenario,
            file,
            entities,
            output,
            format,
            fast,
            dry_run,
            locale,
            include,
            exclude,
            truncate_first,
        } => {
            // Dry-run doesn't need a URL — we report the plan without connecting.
            if !dry_run {
                let _ = require_url(cli.url.clone())?;
            }
            let url = cli.url.unwrap_or_default();
            generate::run(
                generate::Args {
                    seed,
                    rows,
                    scenario,
                    file,
                    entities,
                    output,
                    format,
                    fast,
                    dry_run,
                    locale,
                    include,
                    exclude,
                    truncate_first,
                },
                &url,
            )
            .await
        }
        Commands::Reset {
            confirm,
            only,
            cascade,
        } => {
            let url = require_url(cli.url)?;
            reset::run(
                reset::Args {
                    confirm,
                    only,
                    cascade,
                },
                &url,
            )
            .await
        }
        Commands::Validate { file: _ } => {
            anyhow::bail!("`validate` is not yet implemented (scenario engine pending)");
        }
        Commands::McpServer { transport, port } => match transport.as_str() {
            "stdio" => crate::mcp::run_stdio().await,
            "http" => {
                let _ = port;
                anyhow::bail!("`mcp-server --transport http` not yet implemented (stdio only)");
            }
            other => anyhow::bail!("unknown --transport `{other}` (expected stdio or http)"),
        },
        Commands::Completions { shell } => {
            let mut cmd = Cli::command();
            clap_complete::generate(shell, &mut cmd, "seedgen", &mut std::io::stdout());
            Ok(())
        }
    }
}

fn require_url(url: Option<String>) -> anyhow::Result<String> {
    url.ok_or_else(|| {
        anyhow::anyhow!("no database URL provided — pass --url <URL> or set DATABASE_URL env var")
    })
}
