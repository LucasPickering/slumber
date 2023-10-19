#![deny(clippy::all)]
#![feature(associated_type_defaults)]
#![feature(error_iter)]
#![feature(iterator_try_collect)]
#![feature(try_blocks)]

mod config;
#[cfg(test)]
mod factory;
mod http;
mod template;
mod tui;
mod util;

use crate::{
    config::RequestCollection,
    http::{HttpEngine, Repository},
    template::{Prompt, Prompter, TemplateContext},
    tui::Tui,
    util::find_by,
};
use anyhow::Context;
use clap::Parser;
use indexmap::IndexMap;
use std::{
    error::Error,
    path::{Path, PathBuf},
    str::FromStr,
};
use tracing::error;
use tracing_subscriber::{filter::EnvFilter, prelude::*};

#[derive(Debug, Parser)]
#[clap(
    author,
    version,
    about,
    long_about = "Configurable REST client with both TUI and CLI interfaces"
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

#[derive(Clone, Debug, clap::Subcommand)]
enum Subcommand {
    /// Execute a single request
    #[clap(aliases=&["req", "rq"])]
    Request {
        /// ID of the request to execute
        request_id: String,

        /// ID of the profile to pull template values from
        #[clap(long = "profile", short)]
        profile: Option<String>,

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

#[tokio::main]
async fn main() {
    // Global initialization
    initialize_tracing().unwrap();
    let args = Args::parse();
    // This won't panic at the failure site because it can also be called
    // mid-TUI execution
    let (collection_file, collection) =
        RequestCollection::load(args.collection.as_deref())
            .await
            .expect("Error loading collection");

    // Select mode based on whether request ID(s) were given
    match args.subcommand {
        // Run the TUI
        None => {
            Tui::start(collection_file.to_owned(), collection);
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
            profile,
            overrides,
            dry_run,
        } => {
            // Find profile and recipe by ID
            let profile = profile
                .map(|profile| {
                    Ok::<_, anyhow::Error>(
                        find_by(
                            collection.profiles,
                            |e| &e.id,
                            &profile,
                            "No profile with ID",
                        )?
                        .data,
                    )
                })
                .transpose()?
                .unwrap_or_default();
            let recipe = find_by(
                collection.requests,
                |r| &r.id,
                &request_id,
                "No request recipe with ID",
            )?;

            // Build the request
            let repository = Repository::load()?;
            let overrides: IndexMap<_, _> = overrides.into_iter().collect();
            let request = HttpEngine::build_request(
                &recipe,
                &TemplateContext {
                    profile,
                    overrides,
                    chains: collection.chains,
                    repository: repository.clone(),
                    prompter: Box::new(CliPrompter),
                },
            )
            .await?;

            if dry_run {
                println!("{:#?}", request);
            } else {
                // Run the request
                let http_engine = HttpEngine::new(repository);
                let record = http_engine.send(request).await?;

                // Print response
                print!("{}", record.response.body);
            }
            Ok(())
        }
    }
}

/// Set up tracing to log to a file
fn initialize_tracing() -> anyhow::Result<()> {
    let directory = Path::new("./log/");
    std::fs::create_dir_all(directory)
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

/// CLI doesn't support prompting (yet). Just tell the user to use a command
/// line arg instead.
#[derive(Debug)]
struct CliPrompter;

impl Prompter for CliPrompter {
    fn prompt(&self, _prompt: Prompt) {
        // TODO allow prompts in CLI
        error!(
            "Prompting not supported in CLI. \
            Try `--override` to pass the value via command line argument."
        );
    }
}
