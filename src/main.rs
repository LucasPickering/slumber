#![forbid(unsafe_code)]
#![deny(clippy::all)]

mod cli;
mod collection;
mod db;
mod http;
mod template;
#[cfg(test)]
mod test_util;
mod tui;
mod util;

use crate::{
    cli::CliCommand,
    tui::Tui,
    util::paths::{DataDirectory, TempDirectory},
};
use clap::Parser;
use std::{fs::File, io, path::PathBuf, process::ExitCode};
use tracing::level_filters::LevelFilter;
use tracing_subscriber::{filter::EnvFilter, fmt::format::FmtSpan, prelude::*};

#[derive(Debug, Parser)]
#[clap(
    author,
    version,
    about,
    long_about = "Configurable HTTP client with both TUI and CLI interfaces"
)]
struct Args {
    #[command(flatten)]
    global: GlobalArgs,
    /// Subcommand to execute. If omitted, run the TUI
    #[command(subcommand)]
    subcommand: Option<CliCommand>,
}

/// Arguments that are available to all subcommands and the TUI
#[derive(Debug, Parser)]
struct GlobalArgs {
    /// Collection file, which defines profiles, recipes, etc. If omitted,
    /// check the current and all parent directories for the following files
    /// (in this order): slumber.yml, slumber.yaml, .slumber.yml, .slumber.yaml
    #[clap(long, short)]
    file: Option<PathBuf>,
    /// Print the path to the log file for this session, before running the
    /// given subcommand
    #[clap(long)]
    log: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<ExitCode> {
    // Global initialization
    let args = Args::parse();
    DataDirectory::init()?;
    TempDirectory::init()?;
    initialize_tracing(args.subcommand.is_some()).unwrap();

    if args.global.log {
        println!("{}", TempDirectory::get().log().display());
    }

    // Select mode based on whether request ID(s) were given
    match args.subcommand {
        // Run the TUI
        None => {
            // This should return the error so we get a full stack trac
            Tui::start(args.global.file).await?;
            Ok(ExitCode::SUCCESS)
        }

        // Execute one request without a TUI
        Some(subcommand) => Ok(subcommand
            .execute(args.global)
            .await
            // Do *not* return the error, because that prints a stack trace
            // which is way too verbose. Just print the error messages instead
            .unwrap_or_else(|error| {
                eprintln!("{error}");
                error
                    .chain()
                    .skip(1)
                    .for_each(|cause| eprintln!("  {cause}"));
                ExitCode::FAILURE
            })),
    }
}

/// Set up tracing to log to a file. Optionally also log to stderr (for CLI
/// usage)
fn initialize_tracing(console_output: bool) -> anyhow::Result<()> {
    let path = TempDirectory::get().log();
    let log_file = File::create(path)?;
    let file_subscriber = tracing_subscriber::fmt::layer()
        .with_file(true)
        .with_line_number(true)
        .with_writer(log_file)
        .with_target(false)
        .with_ansi(false)
        .with_span_events(FmtSpan::NEW)
        .with_filter(EnvFilter::from_default_env());

    // Enable console output for CLI
    let console_subscriber = if console_output {
        Some(
            tracing_subscriber::fmt::layer()
                .with_writer(io::stderr)
                .with_target(false)
                .with_span_events(FmtSpan::NEW)
                .without_time()
                .with_filter(LevelFilter::WARN),
        )
    } else {
        None
    };

    tracing_subscriber::registry()
        .with(file_subscriber)
        .with(console_subscriber)
        .init();
    Ok(())
}
