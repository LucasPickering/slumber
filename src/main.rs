mod config;
mod history;
mod http;
mod template;
mod tui;
mod util;

use crate::{
    config::RequestCollection, http::HttpEngine, template::TemplateContext,
    tui::Tui, util::find_by,
};
use anyhow::Context;
use clap::Parser;
use std::{collections::HashMap, error::Error, path::PathBuf, str::FromStr};
use tracing_subscriber::{filter::EnvFilter, prelude::*};

#[derive(Debug, Parser)]
#[clap(
    author,
    version,
    about,
    long_about = "Configurable REST client with both TUI and CLI interfaces"
)]
struct Args {
    /// Collection file, which defines your environments and recipes. If
    /// omitted, check for the following files in the current directory
    /// (first match will be used): slumber.yml, slumber.yaml, .slumber.yml,
    /// .slumber.yaml
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

        /// List of key=value overrides
        #[clap(
            long = "override",
            short = 'o',
            value_parser = parse_key_val::<String, String>,
        )]
        overrides: Vec<(String, String)>,

        /// Just print the generated request, instead of sending it
        #[clap(long)]
        dry_run: bool,
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
            overrides,
            dry_run,
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

            // Build the request
            let http_engine = HttpEngine::new();
            let overrides: HashMap<_, _> = overrides.into_iter().collect();
            let request = http_engine.build_request(
                recipe,
                &TemplateContext {
                    environment,
                    overrides: Some(&overrides),
                },
            )?;

            if dry_run {
                println!("{:#?}", request);
            } else {
                let response = http_engine.send_request(request).await?;
                print!("{}", response.content);
            }
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

/// Parse a single key=value pair for an argument
fn parse_key_val<T, U>(
    s: &str,
) -> Result<(T, U), Box<dyn Error + Send + Sync + 'static>>
where
    T: FromStr,
    T::Err: Error + Send + Sync + 'static,
    U: FromStr,
    U::Err: Error + Send + Sync + 'static,
{
    let (key, value) = s
        .split_once('=')
        .ok_or_else(|| format!("invalid key=value: no \"=\" found in {s:?}"))?;
    Ok((key.parse()?, value.parse()?))
}
