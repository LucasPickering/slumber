use anyhow::Context;
use slumber_util::{ResultTracedAnyhow, paths};
use std::{
    fs::{File, OpenOptions},
    io,
};
use tracing::level_filters::LevelFilter;
use tracing_subscriber::{fmt::format::FmtSpan, prelude::*};

/// This covers two cases: CLI enabled/TUI disabled, or both enabled. We need
/// the CLI for some TUI features such as the -f flag
#[cfg(feature = "cli")]
#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<std::process::ExitCode> {
    use slumber_cli::Args;
    use std::process::ExitCode;

    // Global initialization
    Args::complete(); // If COMPLETE var is enabled, process will stop here
    let args = Args::parse();

    initialize_tracing(args.global.log_level, args.subcommand.is_some());

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
                slumber_cli::print_error(&error);
                ExitCode::FAILURE
            })),
    }
}

/// TUI is enabled, CLI is disabled (for local TUI dev). There is no command
/// parsing here, so the collection file override is just passed as the first
/// (and only argument), instead of using the -f flag.
#[cfg(all(not(feature = "cli"), feature = "tui"))]
#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    use std::env;
    // Parse log level from the LOG variable
    let level = env::var("LOG")
        .map(|value| value.parse().unwrap())
        .unwrap_or(LevelFilter::OFF);
    initialize_tracing(level, false);
    let collection_file = env::args().nth(1).map(String::into);
    slumber_tui::Tui::start(collection_file).await
}

/// Both disabled - problem!!
#[cfg(all(not(feature = "cli"), not(feature = "tui")))]
#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    Err(anyhow::anyhow!(
        "At least one of the `cli` or `tui` features must be enabled"
    ))
}

/// Set up tracing to a log file, and optionally stderr as well. If there's
/// an error creating the log file, we'll skip that part. This means in the TUI
/// the error (and all other tracing) will never be visible, but that's a
/// problem for another day.
fn initialize_tracing(level_filter: LevelFilter, has_stderr: bool) {
    // Failing to log shouldn't be a fatal crash, so just move on
    let log_file = initialize_log_file()
        .context("Error creating log file")
        .traced()
        .ok();

    let file_subscriber = log_file.map(|log_file| {
        // Include PID
        // https://github.com/tokio-rs/tracing/pull/2655
        tracing_subscriber::fmt::layer()
            .with_file(true)
            .with_line_number(true)
            .with_writer(log_file)
            .with_target(false)
            .with_ansi(false)
            .with_span_events(FmtSpan::NONE)
            // File output can't be lower than warn. There's no good reason to
            // disable file output. If someone passes off/error, they probably
            // just want to set the stderr level.
            .with_filter(level_filter.max(LevelFilter::WARN))
    });

    // Enable console output for CLI. By default logging is off, but it can be
    // turned up with the verbose flag
    let stderr_subscriber = tracing_subscriber::fmt::layer()
        .with_writer(io::stderr)
        .with_target(false)
        .with_span_events(FmtSpan::NONE)
        .without_time()
        // Disable stderr for TUI mode
        .with_filter(if has_stderr {
            level_filter
        } else {
            LevelFilter::OFF
        });

    // Enable tokio-console subscriber when tokio_tracing feature is enabled
    #[cfg(feature = "tokio_tracing")]
    {
        tracing_subscriber::registry()
            .with(file_subscriber)
            .with(stderr_subscriber)
            .with(console_subscriber::spawn())
            .init()
    }
    #[cfg(not(feature = "tokio_tracing"))]
    {
        tracing_subscriber::registry()
            .with(file_subscriber)
            .with(stderr_subscriber)
            .init()
    }
}

/// Create a new log file in a temporary directory. Each file gets a unique name
/// so this won't clobber any old files.
fn initialize_log_file() -> anyhow::Result<File> {
    let path = paths::log_file();
    paths::create_parent(&path)?;
    let log_file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)?;
    Ok(log_file)
}
