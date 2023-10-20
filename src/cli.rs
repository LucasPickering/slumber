use crate::{
    config::{ProfileId, RequestCollection, RequestRecipeId},
    http::{HttpEngine, Repository},
    template::{Prompt, Prompter, TemplateContext},
    util::find_by,
};
use anyhow::Context;
use indexmap::IndexMap;
use std::{
    error::Error,
    fs::File,
    io::{self, Write},
    path::PathBuf,
    str::FromStr,
};
use tracing::error;

/// A non-TUI command
#[derive(Clone, Debug, clap::Subcommand)]
pub enum Subcommand {
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
}

impl Subcommand {
    /// Execute a non-TUI command
    pub async fn execute(
        self,
        collection_file: Option<PathBuf>,
    ) -> anyhow::Result<()> {
        match self {
            Subcommand::Request {
                request_id,
                profile,
                overrides,
                dry_run,
            } => {
                let collection = RequestCollection::load(collection_file)
                    .await
                    .expect("Error loading collection");

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
                    collection.recipes,
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
                            .with_context(|| {
                                format!(
                                    "Error opening collection output file \
                                {output_file:?}"
                                )
                            })?,
                    ),
                    None => Box::new(io::stdout()),
                };
                serde_yaml::to_writer(&mut writer, &collection)?;

                Ok(())
            }
        }
    }
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
