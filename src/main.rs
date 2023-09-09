mod config;
mod http;
mod template;
mod tui;
mod util;

use crate::{
    config::RequestCollection, http::HttpEngine, template::TemplateValues,
    tui::Tui, util::find_by,
};
use anyhow::Context;
use clap::Parser;
use std::path::PathBuf;
use tracing_subscriber::{filter::EnvFilter, prelude::*};

#[derive(Debug, Parser)]
#[clap(
    author,
    version,
    about,
    long_about = "Configurable REST client with both TUI and CLI interfaces"
)]
struct Args {
    #[clap(long, short)]
    collection: Option<PathBuf>,

    /// Subcommand to execute. If omitted, run the TUI
    #[command(subcommand)]
    subcommand: Option<Subcommand>,
}

#[derive(Clone, Debug, clap::Subcommand)]
enum Subcommand {
    /// Execute a single request
    #[clap(aliases=&["req", "rq"])]
    Request {
        /// ID of the request to execute
        request_id: String,
        /// ID of the environment to pull template values from
        #[clap(long = "env", short)]
        environment: Option<String>,
    },
}

/// Error handling procedure:
/// - Unexpected errors (e.g. bugs) should be panics where they occur
/// - Expected errors (e.g. network error, bad input) should be shown to the
/// user. For the CLI, just print it. For the TUI, show a popup.
#[tokio::main]
async fn main() {
    // Global initialization
    initialize_tracing().unwrap();
    let args = Args::parse();
    // This won't panic at the failure site because it can also be called
    // mid-TUI execution
    let collection = RequestCollection::load(args.collection.as_deref())
        .await
        .expect("Error loading collection");

    // Select mode based on whether request ID(s) were given
    match args.subcommand {
        // Run the TUI
        None => {
            Tui::start(collection);
        }

        // Execute one request without a TUI
        Some(subcommand) => {
            if let Err(err) = execute_subcommand(collection, subcommand).await {
                eprintln!("{err:#}");
            }
        }
    }
}

/// Execute a non-TUI command
async fn execute_subcommand(
    collection: RequestCollection,
    subcommand: Subcommand,
) -> anyhow::Result<()> {
    match subcommand {
        Subcommand::Request {
            request_id,
            environment,
        } => {
            // Find environment and recipe by ID
            let environment = match environment {
                Some(id) => Some(
                    &find_by(
                        collection.environments.iter(),
                        |e| &e.id,
                        &id,
                        "No environment with ID",
                    )?
                    .data,
                ),
                None => None,
            };
            let recipe = find_by(
                collection.requests.iter(),
                |r| &r.id,
                &request_id,
                "No request recipe with ID",
            )?;

            // Run the request
            let http_engine = HttpEngine::new();
            let request = http_engine
                .build_request(recipe, &TemplateValues { environment })?;
            let response = http_engine.send_request(request).await?;

            print!("{}", response.content);
            Ok(())
        }
    }
}

/// Set up tracing to log to a file
fn initialize_tracing() -> anyhow::Result<()> {
    let directory = PathBuf::from("./log/");
    std::fs::create_dir_all(directory.clone())
        .context(format!("Error creating log directory {directory:?}"))?;
    let log_path = directory.join("ratatui-app.log");
    let log_file = std::fs::File::create(log_path)?;
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
