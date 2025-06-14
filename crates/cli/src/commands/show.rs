use crate::{GlobalArgs, Subcommand};
use clap::Parser;
use serde::Serialize;
use slumber_config::Config;
use slumber_core::database::Database;
use slumber_util::paths;
use std::process::ExitCode;

/// Print meta information about Slumber (config, collections, etc.)
#[derive(Clone, Debug, Parser)]
pub struct ShowCommand {
    #[command(subcommand)]
    target: ShowTarget,
}

#[derive(Copy, Clone, Debug, clap::Subcommand)]
enum ShowTarget {
    /// Print the path of directories/files that Slumber uses
    Paths {
        /// Print the path for just a single target
        target: Option<PathsTarget>,
    },
    /// Print loaded configuration
    ///
    /// This loads the config and re-stringifies it, so it will print exactly
    /// what Slumber will use in action.
    Config,
    /// Print current request collection
    ///
    /// This loads the collection and re-stringifies it, so it will print
    /// exactly what Slumber will use in action.
    Collection,
}

#[derive(Copy, Clone, Debug, clap::ValueEnum)]
enum PathsTarget {
    Collection,
    Config,
    #[value(name = "db")]
    Database,
    Log,
}

impl Subcommand for ShowCommand {
    async fn execute(self, global: GlobalArgs) -> anyhow::Result<ExitCode> {
        match self.target {
            // Print paths
            ShowTarget::Paths { target: None } => {
                println!("Config: {}", Config::path().display());
                println!("Database: {}", Database::path().display());
                println!("Log file: {}", paths::log_file().display());
                println!(
                    "Collection: {}",
                    global
                        .collection_file()
                        .map(|file| file.to_string())
                        .unwrap_or_else(|error| error.to_string())
                );
            }
            ShowTarget::Paths {
                target: Some(PathsTarget::Config),
            } => {
                println!("{}", Config::path().display());
            }
            ShowTarget::Paths {
                target: Some(PathsTarget::Collection),
            } => {
                println!("{}", global.collection_file()?);
            }
            ShowTarget::Paths {
                target: Some(PathsTarget::Database),
            } => {
                println!("{}", Database::path().display());
            }
            ShowTarget::Paths {
                target: Some(PathsTarget::Log),
            } => {
                println!("{}", paths::log_file().display());
            }

            // Print config
            ShowTarget::Config => {
                let config = Config::load()?;
                println!("{}", to_yaml(&config));
            }
            // Print collection
            ShowTarget::Collection => {
                let collection_file = global.collection_file()?;
                let collection = collection_file.load()?;
                println!("{}", to_yaml(&collection));
            }
        }
        Ok(ExitCode::SUCCESS)
    }
}

fn to_yaml<T: Serialize>(value: &T) -> String {
    // Panic is intentional, indicates a wonky bug
    serde_yaml::to_string(value).expect("Error serializing")
}
