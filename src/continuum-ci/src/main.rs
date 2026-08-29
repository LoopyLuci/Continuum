#![warn(clippy::all)]

mod config;
mod dashboard;
mod pipeline;
mod reporter;
mod runner;
mod types;
mod watcher;

use anyhow::Result;
use clap::{Parser, Subcommand};
use config::CiConfig;
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(
    name = "continuum-ci",
    version,
    about = "Local CI/CD system for Continuum"
)]
struct Cli {
    #[arg(short, long, help = "Path to CI config file")]
    config: Option<PathBuf>,

    #[arg(long, help = "Log level (trace, debug, info, warn, error)")]
    log_level: Option<String>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Run the full pipeline once
    Run {
        #[arg(short, long, help = "Only run specific stages")]
        stages: Vec<String>,

        #[arg(long, help = "Skip file watching after run")]
        no_watch: bool,
    },

    /// Watch for file changes and auto-run pipeline
    Watch {
        #[arg(short, long, help = "Max number of consecutive auto-builds")]
        max_builds: Option<usize>,
    },

    /// Show build history
    Status {
        #[arg(short, long, default_value = "10")]
        limit: usize,

        /// Show details for a specific build
        #[arg(short, long)]
        build: Option<String>,
    },

    /// Show logs for a build
    Logs {
        /// Build ID to show logs for
        build_id: String,

        /// Stream to filter (stdout, stderr, system)
        #[arg(short, long)]
        stream: Option<String>,

        #[arg(short = 'f', long, help = "Follow output")]
        follow: bool,
    },

    /// Start the web dashboard
    Dashboard {
        #[arg(short, long, help = "Dashboard listen address")]
        addr: Option<String>,

        #[arg(short, long, help = "Dashboard port")]
        port: Option<u16>,
    },

    /// Initialize default config file
    Init {
        #[arg(default_value = "continuum-ci.toml")]
        path: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let level = cli.log_level.unwrap_or_else(|| "info".to_string());
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&level));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();

    let config = CiConfig::load(cli.config.as_deref())?;

    match &cli.command {
        Some(Commands::Run {
            stages,
            no_watch: _,
        }) => {
            run_pipeline(&config, stages).await?;
            Ok(())
        }
        Some(Commands::Watch { max_builds }) => {
            watch_and_build(&config, *max_builds).await?;
            Ok(())
        }
        Some(Commands::Status { limit, build }) => {
            show_status(&config, *limit, build).await?;
            Ok(())
        }
        Some(Commands::Logs {
            build_id,
            stream,
            follow,
        }) => {
            show_logs(&config, build_id, stream, *follow).await?;
            Ok(())
        }
        Some(Commands::Dashboard { addr, port }) => {
            start_dashboard_with_config(&config, addr.clone(), *port).await?;
            Ok(())
        }
        Some(Commands::Init { path }) => {
            CiConfig::save_default(path)?;
            println!("Created default CI config: {}", path.display());
            Ok(())
        }
        None => {
            println!("Continuum CI v{}", env!("CARGO_PKG_VERSION"));
            println!("Usage: continuum-ci [COMMAND]");
            println!();
            println!("Commands:");
            println!("  run        Run the full pipeline once");
            println!("  watch      Watch for file changes and auto-run pipeline");
            println!("  status     Show build history");
            println!("  logs       Show logs for a build");
            println!("  dashboard  Start the web dashboard");
            println!("  init       Create default config file");
            Ok(())
        }
    }
}

async fn run_pipeline(config: &CiConfig, _stages: &[String]) -> Result<()> {
    let pipeline = pipeline::Pipeline::new(config.clone());

    let reporter = pipeline.reporter();
    let _reporter_dash = reporter;

    if config.dashboard.enabled {
        let cfg = config.clone();
        let rep = crate::reporter::Reporter::new(config);
        tokio::spawn(async move {
            dashboard::start_dashboard(cfg, rep).await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }

    let build = pipeline.run(types::BuildTrigger::Manual).await?;

    // Wait for build to complete
    loop {
        let status = build.status.read().unwrap().clone();
        print!("\r[{}] {} ...  ", build.id, status.label());
        std::io::Write::flush(&mut std::io::stdout())?;

        if status.is_terminal() {
            println!();
            tracing::info!(build_id = %build.id, status = %status.label(), "Build finished");
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }

    let final_status = build.status.read().unwrap().clone();
    println!("\nBuild {}: {}", build.id, final_status.label());

    Ok(())
}

async fn watch_and_build(config: &CiConfig, max_builds: Option<usize>) -> Result<()> {
    use crate::watcher::FileWatcher;

    let watcher = FileWatcher::new(config)?;
    let pipeline = pipeline::Pipeline::new(config.clone());

    if config.dashboard.enabled {
        let cfg = config.clone();
        let rep = crate::reporter::Reporter::new(config);
        tokio::spawn(async move {
            dashboard::start_dashboard(cfg, rep).await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }

    println!(
        "Watching for changes in {} paths...",
        config.watch.paths.len()
    );
    println!("Press Ctrl+C to stop.");

    let mut build_count = 0usize;
    while let Ok(changes) = watcher.wait_for_changes() {
        build_count += 1;
        if let Some(max) = max_builds {
            if build_count > max {
                println!("Reached max builds ({}), stopping", max);
                break;
            }
        }

        println!("\nChanges detected ({} files):", changes.len());
        for change in changes.iter().take(5) {
            println!("  {}", change);
        }
        if changes.len() > 5 {
            println!("  ... and {} more", changes.len() - 5);
        }

        let build = pipeline
            .run(types::BuildTrigger::FileWatch { paths: changes })
            .await?;

        let build_id = build.id.clone();
        let final_status = build.status.clone();

        // Wait for completion
        loop {
            let status = final_status.read().unwrap().clone();
            print!("\r[{}] {} ...", build_id, status.label());
            std::io::Write::flush(&mut std::io::stdout())?;
            if status.is_terminal() {
                println!();
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
    }

    Ok(())
}

async fn show_status(config: &CiConfig, limit: usize, build_id: &Option<String>) -> Result<()> {
    let reporter = crate::reporter::Reporter::new(config);

    if let Some(id) = build_id {
        if let Some(build) = reporter.get_build(id)? {
            println!("Build: {}", build.id);
            println!("Status: {}", build.status.label());
            println!("Pipeline: {}", build.pipeline_name);
            println!("Branch: {}", build.branch);
            println!("Commit: {}", build.commit);
            println!("Started: {}", build.started_at);
            println!(
                "Duration: {}",
                crate::types::format_duration(build.duration_ms)
            );
            println!();

            if !build.stages.is_empty() {
                println!("Stages:");
                for stage in &build.stages {
                    let icon = match stage.status {
                        types::BuildStatus::Success => "✓",
                        types::BuildStatus::Failure => "✗",
                        types::BuildStatus::Running => "▶",
                        types::BuildStatus::Pending => "·",
                        types::BuildStatus::Cancelled => "⊘",
                        types::BuildStatus::Timeout => "⚠",
                    };
                    println!(
                        "  {} {} ({})",
                        icon,
                        stage.name,
                        crate::types::format_duration(stage.duration_ms)
                    );
                }
            }
        } else {
            println!("Build '{}' not found", id);
        }
    } else {
        let builds = reporter.list_builds(limit)?;
        if builds.is_empty() {
            println!("No builds yet");
            return Ok(());
        }

        println!(
            "{:<10} {:<12} {:<8} {:<20} FINISHED",
            "BUILD", "STATUS", "DURATION", "BRANCH"
        );
        println!("{}", "-".repeat(80));
        for build in &builds {
            println!(
                "{:<10} {:<12} {:<8} {:<20} {}",
                build.id,
                build.status.label(),
                crate::types::format_duration(build.duration_ms),
                build.branch,
                build
                    .finished_at
                    .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_else(|| "—".to_string()),
            );
        }
        println!("\nTotal: {} builds", builds.len());
    }

    Ok(())
}

async fn show_logs(
    config: &CiConfig,
    build_id: &str,
    stream_filter: &Option<String>,
    _follow: bool,
) -> Result<()> {
    let reporter = crate::reporter::Reporter::new(config);

    if let Some(build) = reporter.get_build(build_id)? {
        for stage in &build.stages {
            println!("\n── {} ──", stage.name);
            if stage.output.is_empty() {
                println!("  (no output captured)");
            } else {
                for line in &stage.output {
                    if let Some(filter) = stream_filter {
                        let stream_name = format!("{:?}", line.stream).to_lowercase();
                        if !stream_name.contains(&filter.to_lowercase()) {
                            continue;
                        }
                    }
                    let prefix = match line.stream {
                        types::StreamType::Stdout => "  ",
                        types::StreamType::Stderr => "E ",
                        types::StreamType::System => "# ",
                    };
                    println!("{}{}", prefix, line.text);
                }
            }
        }

        if build.status == types::BuildStatus::Running {
            println!("\nBuild is still running. Use --follow to stream new output.");
        }
    } else {
        println!("Build '{}' not found", build_id);
    }

    Ok(())
}

async fn start_dashboard_with_config(
    config: &CiConfig,
    addr: Option<String>,
    port: Option<u16>,
) -> Result<()> {
    let mut config = config.clone();
    if let Some(a) = addr {
        config.dashboard.listen_addr = a;
    }
    if let Some(p) = port {
        config.dashboard.port = p;
    }

    let reporter = crate::reporter::Reporter::new(&config);
    dashboard::start_dashboard(config, reporter).await;
    Ok(())
}
