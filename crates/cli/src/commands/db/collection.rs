use crate::{
    GlobalArgs, Subcommand, completions::complete_collection_specifier,
    util::print_table,
};
use clap::Parser;
use slumber_core::database::{CollectionId, Database};
use std::{
    fmt::{self, Display},
    path::PathBuf,
    process::ExitCode,
    str::FromStr,
};

/// View and modify request collection metadata
#[derive(Clone, Debug, Parser)]
pub struct DbCollectionCommand {
    #[command(subcommand)]
    subcommand: DbCollectionSubcommand,
}

#[derive(Clone, Debug, clap::Subcommand)]
enum DbCollectionSubcommand {
    /// List all known request collections
    #[command(visible_alias = "ls")]
    List,
    /// Delete all history for a collection
    ///
    /// This will delete all record of the collection from the history database.
    /// It will NOT delete the collection YAML file.
    #[command(visible_alias = "rm")]
    Delete {
        /// Path or ID of the collection(s) to delete
        #[clap(num_args = 1..)]
        collection: Vec<CollectionSpecifier>,
    },
    /// Move all data from one collection to another.
    ///
    /// The data from the source collection will be merged into the target
    /// collection, then all traces of the source collection will be deleted!
    Migrate {
        /// Path or ID of the collection to migrate *from*
        #[clap(add = complete_collection_specifier())]
        from: CollectionSpecifier,
        /// Path or ID of the collection to migrate *into*
        to: CollectionSpecifier,
    },
}

impl Subcommand for DbCollectionCommand {
    async fn execute(self, _global: GlobalArgs) -> anyhow::Result<ExitCode> {
        let database = Database::load()?;
        match self.subcommand {
            DbCollectionSubcommand::List => {
                let rows = database
                    .get_collections()?
                    .into_iter()
                    .map(|collection| {
                        [
                            collection.id.to_string(),
                            collection.path.display().to_string(),
                            collection.name.unwrap_or_default(),
                        ]
                    })
                    .collect::<Vec<_>>();
                print_table(["ID", "Path", "Name"], &rows);
            }
            DbCollectionSubcommand::Delete { collection } => {
                for collection in collection {
                    let id = collection.to_id(&database)?;
                    database.delete_collection(id)?;
                    println!("Deleted collection {id}");
                }
            }
            DbCollectionSubcommand::Migrate { from, to } => {
                let from_id = from.to_id(&database)?;
                let to_id = to.to_id(&database)?;
                database.merge_collections(from_id, to_id)?;
                println!("Migrated {from} into {to}");
            }
        }
        Ok(ExitCode::SUCCESS)
    }
}

/// Specify a collection by file path or ID
#[derive(Clone, Debug)]
enum CollectionSpecifier {
    Id(CollectionId),
    Path(PathBuf),
}

impl CollectionSpecifier {
    fn to_id(&self, database: &Database) -> anyhow::Result<CollectionId> {
        match self {
            Self::Id(id) => {
                // Ensure the ID is actually in the DB
                database.get_collection_metadata(*id)?;
                Ok(*id)
            }
            Self::Path(path) => database
                .get_collection_id(path)
                .map_err(anyhow::Error::from),
        }
    }
}

impl Display for CollectionSpecifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Id(id) => write!(f, "{id}"),
            Self::Path(path) => write!(f, "{}", path.display()),
        }
    }
}

impl FromStr for CollectionSpecifier {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(s.parse::<CollectionId>()
            .map(Self::Id)
            .unwrap_or_else(|_| Self::Path(PathBuf::from(s))))
    }
}
