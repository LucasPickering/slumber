use crate::{
    cli::Subcommand, collection::CollectionFile, db::Database,
    util::paths::DataDirectory, GlobalArgs,
};
use clap::Parser;
use serde::Serialize;
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
    /// Print current request collection
    Collection,
}

impl Subcommand for ShowCommand {
    async fn execute(self, global: GlobalArgs) -> anyhow::Result<ExitCode> {
        match self.target {
            ShowTarget::Paths => {
                let collection_path =
                    CollectionFile::try_path(None, global.file);
                println!("Data directory: {}", DataDirectory::root());
                println!("Log file: {}", DataDirectory::log());
                println!("Database: {}", Database::path());
                println!(
                    "Collection: {}",
                    collection_path
                        .as_deref()
                        .map(Path::to_string_lossy)
                        .unwrap_or_else(|error| Cow::Owned(error.to_string()))
                )
            }
            ShowTarget::Collection => {
                let collection_path =
                    CollectionFile::try_path(None, global.file)?;
                let collection_file =
                    CollectionFile::load(collection_path).await?;
                println!("{}", to_yaml(&collection_file.collection));
            }
        }
        Ok(ExitCode::SUCCESS)
    }
}

fn to_yaml<T: Serialize>(value: &T) -> String {
    // Panic is intentional, indicates a wonky bug
    serde_yaml::to_string(value).expect("Error serializing")
}
