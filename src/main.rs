#![forbid(unsafe_code)]
#![deny(clippy::all)]

mod cli;
mod collection;
mod db;
#[cfg(test)]
mod factory;
mod http;
mod template;
mod tui;
mod util;

use crate::{
    cli::Subcommand, collection::RequestCollection, tui::Tui, util::Directory,
};
use clap::Parser;
use std::{fs::File, path::PathBuf};
use tracing_subscriber::{filter::EnvFilter, prelude::*};

#[derive(Debug, Parser)]
#[clap(
    author,
    version,
    about,
    long_about = "Configurable HTTP client with both TUI and CLI interfaces"
)]
struct Args {
    /// Collection file, which defines your profiless and recipes. If omitted,
    /// check for the following files in the current directory (first match
    /// will be used): slumber.yml, slumber.yaml, .slumber.yml, .slumber.yaml
    #[clap(long, short)]
    collection: Option<PathBuf>,

    /// Subcommand to execute. If omitted, run the TUI
    #[command(subcommand)]
    subcommand: Option<Subcommand>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Global initialization
    initialize_tracing().unwrap();
    let args = Args::parse();

    // Select mode based on whether request ID(s) were given
    match args.subcommand {
        // Run the TUI
        None => {
            let collection_path = RequestCollection::try_path(args.collection)?;
            Tui::start(collection_path).await;
            Ok(())
        }

        // Execute one request without a TUI
        Some(subcommand) => subcommand.execute(args.collection).await,
    }
}

/// Set up tracing to log to a file
fn initialize_tracing() -> anyhow::Result<()> {
    let path = Directory::log().create()?.join("slumber.log");
    let log_file = File::create(path)?;
    let file_subscriber = tracing_subscriber::fmt::layer()
        .with_file(true)
        .with_line_number(true)
        .with_writer(log_file)
        .with_target(false)
        .with_ansi(false)
        .with_filter(EnvFilter::from_default_env());
    tracing_subscriber::registry().with(file_subscriber).init();
    Ok(())
}
