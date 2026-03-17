use slumber_cli::Args;
use slumber_tui::Tui;
use slumber_util::initialize_tracing;
use std::process::ExitCode;

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<std::process::ExitCode> {
    // Global initialization
    Args::complete(); // If COMPLETE var is enabled, process will stop here
    let args = Args::parse();

    initialize_tracing(args.global.log_level, args.subcommand.is_some());

    // Select mode based on whether request ID(s) were given
    match args.subcommand {
        // Run the TUI
        None => {
            // This should return the error so we get a full stack trace
            Tui::start(args.global.file).await?;
            Ok(ExitCode::SUCCESS)
        }

        // Run the CLI
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
