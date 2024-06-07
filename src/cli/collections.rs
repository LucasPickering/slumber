use crate::{cli::Subcommand, db::Database, GlobalArgs};
use clap::Parser;
use std::{path::PathBuf, process::ExitCode};

/// View and modify request collection metadata
#[derive(Clone, Debug, Parser)]
pub struct CollectionsCommand {
    #[command(subcommand)]
    subcommand: CollectionsSubcommand,
}

#[derive(Clone, Debug, clap::Subcommand)]
enum CollectionsSubcommand {
    /// List all known request collections
    #[command(visible_alias = "ls")]
    List,
    /// Move all data from one collection to another.
    ///
    /// The data from the source collection will be merged into the target
    /// collection, then all traces of the source collection will be deleted!
    Migrate {
        /// The path the collection to migrate *from*
        from: PathBuf,
        /// The path the collection to migrate *into*
        to: PathBuf,
    },
}

impl Subcommand for CollectionsCommand {
    async fn execute(self, _global: GlobalArgs) -> anyhow::Result<ExitCode> {
        let database = Database::load()?;
        match self.subcommand {
            CollectionsSubcommand::List => {
                for path in database.collections()? {
                    println!("{}", path.display());
                }
            }
            CollectionsSubcommand::Migrate { from, to } => {
                database.merge_collections(&from, &to)?;
                println!("Migrated {} into {}", from.display(), to.display());
            }
        }
        Ok(ExitCode::SUCCESS)
    }
}
