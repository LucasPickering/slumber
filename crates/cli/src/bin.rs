//! Test-only binary for CLI integration tests. Unfortunately I can't figure out
//! how to make this compile only in `cfg(test)`, so its dependencies (tokio)
//! can't be in dev-dependencies. This doesn't actually add anything to the
//! final dependency tree though.

use slumber_cli::Args;
use std::process::ExitCode;

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let args = Args::parse();
    args.subcommand
        .expect("Subcommand required for CLI tests")
        .execute(args.global)
        .await
        .unwrap_or_else(|error| {
            eprintln!("{error}");
            error
                .chain()
                .skip(1)
                .for_each(|cause| eprintln!("  {cause}"));
            ExitCode::FAILURE
        })
}
