use crate::{GlobalArgs, Subcommand};
use clap::Parser;
use slumber_config::Config;
use slumber_core::{
    collection::{CollectionFile, LoadedCollection},
    database::Database,
};
use slumber_util::paths;
use std::{borrow::Cow, process::ExitCode};

/// Print meta information about Slumber (config, collections, etc.)
#[derive(Clone, Debug, Parser)]
pub struct ShowCommand {
    #[command(subcommand)]
    target: ShowTarget,
}

#[derive(Copy, Clone, Debug, clap::Subcommand)]
enum ShowTarget {
    /// Print the path of all directories/files that Slumber uses
    Paths,
    /// Print loaded configuration
    Config,
    /// Print current request collection
    Collection,
}

impl Subcommand for ShowCommand {
    async fn execute(self, global: GlobalArgs) -> anyhow::Result<ExitCode> {
        match self.target {
            ShowTarget::Paths => {
                let collection_file = global.collection_file();
                println!("Config: {}", Config::path().display());
                println!("Database: {}", Database::path().display());
                println!("Log file: {}", paths::log_file().display());
                println!(
                    "Collection: {}",
                    collection_file
                        .as_ref()
                        .map(|file| file.path().to_string_lossy())
                        .unwrap_or_else(|error| Cow::Owned(error.to_string()))
                )
            }
            ShowTarget::Config => {
                let config = Config::load()?;
                println!("{}", serde_yaml::to_string(&config).unwrap());
            }
            ShowTarget::Collection => {
                let collection_file = CollectionFile::new(global.file)?;
                let LoadedCollection { collection, .. } =
                    collection_file.load()?;
                println!(
                    "{:#}",
                    petitscript::serde::to_value(&collection).unwrap()
                );
            }
        }
        Ok(ExitCode::SUCCESS)
    }
}
