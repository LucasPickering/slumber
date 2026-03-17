use slumber_tui::Tui;
use slumber_util::initialize_tracing;
use std::env;
use tracing::level_filters::LevelFilter;

/// DEVELOPMENT ONLY: Start the TUI
///
/// Because this doesn't use the CLI for arg parsing, it has minimal arg
/// support:
/// - It accepts one optional arg, which overrides the collection file path
/// - The `LOG` env var controls the log level
#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    // Parse log level from the LOG variable
    let level = env::var("LOG")
        .map(|value| value.parse().unwrap())
        .unwrap_or(LevelFilter::INFO);
    initialize_tracing(level, false);
    let collection_file = env::args().nth(1).map(String::into);
    Tui::start(collection_file).await
}
