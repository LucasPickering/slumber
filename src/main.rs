#![forbid(unsafe_code)]
#![deny(clippy::all)]

use slumber_cli::Args;
use slumber_core::util::{DataDirectory, TempDirectory};
use slumber_tui::Tui;
use std::{fs::File, io, process::ExitCode};
use tracing::level_filters::LevelFilter;
use tracing_subscriber::{filter::Targets, fmt::format::FmtSpan, prelude::*};

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
    // Basically a minimal version of EnvFilter that doesn't require regexes
    // https://github.com/tokio-rs/tracing/issues/1436#issuecomment-918528013
    let targets: Targets = std::env::var("RUST_LOG")
        .ok()
        .and_then(|env| env.parse().ok())
        .unwrap_or_default();
    let file_subscriber = tracing_subscriber::fmt::layer()
        .with_file(true)
        .with_line_number(true)
        .with_writer(log_file)
        .with_target(false)
        .with_ansi(false)
        .with_span_events(FmtSpan::NEW)
        .with_filter(targets);

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
