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
        /// Path or ID the collection to migrate *from*
        #[clap(add = complete_collection_specifier())]
        from: CollectionSpecifier,
        /// Path or ID the collection to migrate *into*
        #[clap(add = complete_collection_specifier())]
        to: CollectionSpecifier,
    },
}

impl Subcommand for CollectionsCommand {
    async fn execute(self, _global: GlobalArgs) -> anyhow::Result<ExitCode> {
        let database = Database::load()?;
        match self.subcommand {
            CollectionsSubcommand::List => {
                let rows = database
                    .collections()?
                    .into_iter()
                    .map(|collection| {
                        [
                            collection.path.display().to_string(),
                            collection.name.unwrap_or_default(),
                        ]
                    })
                    .collect::<Vec<_>>();
                print_table(["Path", "Name"], &rows);
            }
            CollectionsSubcommand::Migrate { from, to } => {
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
            Self::Id(id) => Ok(*id),
            Self::Path(path) => database.get_collection_id(path),
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

impl Display for CollectionSpecifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Id(id) => write!(f, "{id}"),
            Self::Path(path) => write!(f, "{}", path.display()),
        }
    }
}
