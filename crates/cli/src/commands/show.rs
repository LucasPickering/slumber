use crate::{GlobalArgs, Subcommand};
use clap::Parser;
use serde::Serialize;
use slumber_config::Config;
use slumber_core::{
    collection::CollectionFile, db::Database, lua::LuaVm, util::paths,
};
use std::{borrow::Cow, path::Path, process::ExitCode};

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
                let collection_path =
                    CollectionFile::try_path(None, global.file);
                println!("Config: {}", Config::path().display());
                println!("Database: {}", Database::path().display());
                println!("Log file: {}", paths::log_file().display());
                println!(
                    "Collection: {}",
                    collection_path
                        .as_deref()
                        .map(Path::to_string_lossy)
                        .unwrap_or_else(|error| Cow::Owned(error.to_string()))
                )
            }
            ShowTarget::Config => {
                let config = Config::load()?;
                println!("{}", to_yaml(&config));
            }
            ShowTarget::Collection => {
                let collection_path =
                    CollectionFile::try_path(None, global.file)?;
                let collection =
                    LuaVm::new().load_collection(&collection_path)?;
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
