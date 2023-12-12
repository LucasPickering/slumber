use crate::{
    collection::{ProfileId, RequestCollection, RequestRecipeId},
    db::Database,
    http::{HttpEngine, RequestBuilder},
    template::{Prompt, Prompter, TemplateContext},
    util::{Directory, ResultExt},
};
use anyhow::{anyhow, Context};
use dialoguer::{Input, Password};
use indexmap::IndexMap;
use std::{
    error::Error,
    fs::File,
    io::{self, Write},
    path::PathBuf,
    str::FromStr,
};

/// A non-TUI command
#[derive(Clone, Debug, clap::Subcommand)]
pub enum Subcommand {
    // TODO Break this apart into multiple files
    /// Execute a single request
    #[clap(aliases=&["req", "rq"])]
    Request {
        /// ID of the request recipe to execute
        request_id: RequestRecipeId,

        /// ID of the profile to pull template values from
        #[clap(long = "profile", short)]
        profile: Option<ProfileId>,

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

    /// Generate a slumber request collection from an external format
    #[clap(name = "import-experimental")]
    Import {
        /// Collection to import
        input_file: PathBuf,
        /// Destination for the new slumber collection file. Omit to print to
        /// stdout.
        output_file: Option<PathBuf>,
    },

    /// View and modify request collection history
    Collections {
        #[command(subcommand)]
        subcommand: CollectionsSubcommand,
    },

    /// Show meta information about slumber
    Show {
        #[command(subcommand)]
        target: ShowTarget,
    },
}

#[derive(Copy, Clone, Debug, clap::Subcommand)]
pub enum ShowTarget {
    /// Show the directory where slumber stores data and log files
    Dir,
}

#[derive(Clone, Debug, clap::Subcommand)]
pub enum CollectionsSubcommand {
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

impl Subcommand {
    /// Execute a non-TUI command
    pub async fn execute(
        self,
        collection_override: Option<PathBuf>,
    ) -> anyhow::Result<()> {
        match self {
            Subcommand::Request {
                request_id,
                profile,
                overrides,
                dry_run,
            } => {
                let collection_path =
                    RequestCollection::try_path(collection_override)?;
                let database =
                    Database::load()?.into_collection(&collection_path)?;
                let mut collection =
                    RequestCollection::load(collection_path).await?;

                // Find profile and recipe by ID
                // TODO include list of valid IDs in error msgs here
                let profile_data = profile
                    .map::<anyhow::Result<_>, _>(|id| {
                        let profile =
                            collection.profiles.swap_remove(&id).ok_or_else(
                                || anyhow!("No profile with ID `{id}`"),
                            )?;
                        Ok(profile.data)
                    })
                    .transpose()?
                    .unwrap_or_default();
                let recipe =
                    collection.recipes.swap_remove(&request_id).ok_or_else(
                        || anyhow!("No request with ID `{request_id}`"),
                    )?;

                // Build the request
                let overrides: IndexMap<_, _> = overrides.into_iter().collect();
                let request = RequestBuilder::new(
                    recipe,
                    TemplateContext {
                        profile: profile_data,
                        overrides,
                        chains: collection.chains,
                        database: database.clone(),
                        prompter: Box::new(CliPrompter),
                    },
                )
                .build()
                .await?;

                if dry_run {
                    println!("{:#?}", request);
                } else {
                    // Run the request
                    let http_engine = HttpEngine::new(database);
                    let record = http_engine.send(request).await?;

                    // Print response
                    print!("{}", record.response.body.text());
                }
                Ok(())
            }

            Subcommand::Import {
                input_file,
                output_file,
            } => {
                // Load the input
                let collection = RequestCollection::from_insomnia(&input_file)?;

                // Write the output
                let mut writer: Box<dyn Write> = match output_file {
                    Some(output_file) => Box::new(
                        File::options()
                            .create(true)
                            .truncate(true)
                            .write(true)
                            .open(&output_file)
                            .context(format!(
                                "Error opening collection output file \
                                {output_file:?}"
                            ))?,
                    ),
                    None => Box::new(io::stdout()),
                };
                serde_yaml::to_writer(&mut writer, &collection)?;

                Ok(())
            }

            Subcommand::Collections { subcommand } => subcommand.execute(),

            Subcommand::Show { target } => {
                match target {
                    ShowTarget::Dir => println!("{}", Directory::root()),
                }
                Ok(())
            }
        }
    }
}

impl CollectionsSubcommand {
    fn execute(self) -> anyhow::Result<()> {
        let database = Database::load()?;
        match self {
            CollectionsSubcommand::List => {
                for path in database.get_collections()? {
                    println!("{}", path.display());
                }
            }
            CollectionsSubcommand::Migrate { from, to } => {
                database.merge_collections(&from, &to)?;
                println!("Migrated {} into {}", from.display(), to.display());
            }
        }
        Ok(())
    }
}

/// Prompt the user for input on the CLI
#[derive(Debug)]
struct CliPrompter;

impl Prompter for CliPrompter {
    fn prompt(&self, prompt: Prompt) {
        // This will implicitly queue the prompts by blocking the main thread.
        // Since the CLI has nothing else to do while waiting on a response,
        // that's fine.
        let result = if prompt.sensitive() {
            Password::new().with_prompt(prompt.label()).interact()
        } else {
            Input::new().with_prompt(prompt.label()).interact()
        };

        // If we failed to read the value, print an error and report nothing
        if let Ok(value) =
            result.context("Error reading value from prompt").traced()
        {
            prompt.respond(value);
        }
    }
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
        .ok_or_else(|| format!("invalid key=value: no \"=\" found in `{s}`"))?;
    Ok((key.parse()?, value.parse()?))
}
