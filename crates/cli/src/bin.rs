use slumber_cli::Args;
use slumber_util::initialize_tracing;
use std::process::ExitCode;

/// DEVELOPMENT ONLY: Run the CLI
#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let args = Args::parse();
    initialize_tracing(args.global.log_level, true);

    // Select mode based on whether request ID(s) were given
    match args.subcommand {
        None => {
            eprintln!("TUI not available; pass a subcommand");
            ExitCode::FAILURE
        }

        Some(subcommand) => subcommand
            .execute(args.global)
            .await
            .unwrap_or_else(|error| {
                slumber_cli::print_error(&error);
                ExitCode::FAILURE
            }),
    }
}
