use clap::Parser;
use slumber_fs::Args;
use slumber_util::initialize_tracing;

/// DEVELOPMENT ONLY: Run the filesystem frontend
#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args = Args::parse();
    initialize_tracing(args.log_level, true);
    if let Err(error) = slumber_fs::run(args).await {
        eprintln!("{error}");
    }
}
