#![forbid(unsafe_code)]
#![deny(clippy::all)]

use anyhow::Context;
use slumber_cli::Args;
use slumber_core::util::{paths, ResultTraced};
use std::{
    fs::{self, File, OpenOptions},
    io,
    process::ExitCode,
};
use tracing::level_filters::LevelFilter;
use tracing_subscriber::{filter::Targets, fmt::format::FmtSpan, prelude::*};

#[tokio::main]
async fn main() -> anyhow::Result<ExitCode> {
    // Global initialization
    Args::complete(); // If COMPLETE var is enabled, process will stop here
    let args = Args::parse();

    initialize_tracing(args.subcommand.is_some());

    // Select mode based on whether request ID(s) were given
    match args.subcommand {
        // Run the TUI. TUI can be disabled so we don't have to compile it while
        // testing the CLI
        #[cfg(feature = "tui")]
        None => {
            // This should return the error so we get a full stack trace
            slumber_tui::Tui::start(args.global.file).await?;
            Ok(ExitCode::SUCCESS)
        }
        #[cfg(not(feature = "tui"))]
        None => Err(anyhow::anyhow!("TUI feature is disabled")),

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

/// Set up tracing to a log file, and optionally the console as well. If there's
/// an error creating the log file, we'll skip that part. This means in the TUI
/// the error (and all other tracing) will never be visible, but that's a
/// problem for another day.
fn initialize_tracing(console_output: bool) {
    // Failing to log shouldn't be a fatal crash, so just move on
    let log_file = initialize_log_file()
        .context("Error creating log file")
        .traced()
        .ok();

    // Basically a minimal version of EnvFilter that doesn't require regexes
    // https://github.com/tokio-rs/tracing/issues/1436#issuecomment-918528013
    let targets: Targets = std::env::var("RUST_LOG")
        .ok()
        .and_then(|env| env.parse().ok())
        .unwrap_or_else(|| {
            Targets::new().with_target("slumber", LevelFilter::WARN)
        });
    let file_subscriber = log_file.map(|log_file| {
        // Include PID
        // https://github.com/tokio-rs/tracing/pull/2655
        tracing_subscriber::fmt::layer()
            .with_file(true)
            .with_line_number(true)
            .with_writer(log_file)
            .with_target(false)
            .with_ansi(false)
            .with_span_events(FmtSpan::NEW)
            .with_filter(targets)
    });

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
}

/// Create the log file. If it already exists, make sure it's not over a max
/// size. If it is, move it to a backup path and nuke whatever might be in the
/// backup path.
fn initialize_log_file() -> anyhow::Result<File> {
    const MAX_FILE_SIZE: u64 = 1000 * 1000; // 1MB
    let path = paths::log_file();
    paths::create_parent(&path)?;

    if fs::metadata(&path)
        .map_or(false, |metadata| metadata.len() > MAX_FILE_SIZE)
    {
        // Rename new->old, overwriting old. If that fails, just delete new so
        // it doesn't grow indefinitely. Failure shouldn't stop us from logging
        // though
        let _ = fs::rename(&path, paths::log_file_old())
            .or_else(|_| fs::remove_file(&path));
    }

    let log_file = OpenOptions::new().create(true).append(true).open(path)?;
    Ok(log_file)
}
