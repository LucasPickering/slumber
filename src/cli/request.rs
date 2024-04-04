use crate::{
    cli::Subcommand,
    collection::{CollectionFile, ProfileId, RecipeId},
    config::Config,
    db::{CollectionDatabase, Database},
    http::{HttpEngine, RecipeOptions, Request, RequestBuilder},
    template::{Prompt, Prompter, TemplateContext},
    util::{MaybeStr, ResultExt},
    GlobalArgs,
};
use anyhow::{anyhow, Context};
use async_trait::async_trait;
use clap::Parser;
use dialoguer::{console::Style, Input, Password};
use indexmap::IndexMap;
use itertools::Itertools;
use reqwest::header::HeaderMap;
use std::{
    error::Error,
    fmt::{self, Display, Formatter},
    io::{self, Write},
    process::ExitCode,
    str::FromStr,
};

/// Exit code to return when `exit_status` flag is set and the HTTP response has
/// an error status code
const HTTP_ERROR_EXIT_CODE: u8 = 2;

/// Execute a single request, and print its response
#[derive(Clone, Debug, Parser)]
#[clap(aliases=&["req", "rq"])]
pub struct RequestCommand {
    #[clap(flatten)]
    build_request: BuildRequestCommand,

    /// Print HTTP response status
    #[clap(long)]
    status: bool,

    /// Print HTTP request and response headers
    #[clap(long)]
    headers: bool,

    /// Do not print HTTP response body
    #[clap(long)]
    no_body: bool,

    /// Set process exit code based on HTTP response status. If the status is
    /// <400, exit code is 0. If it's >=400, exit code is 2.
    #[clap(long)]
    exit_status: bool,

    /// Just print the generated request, instead of sending it
    #[clap(long)]
    dry_run: bool,
}

/// A helper for any subcommand that needs to build requests. This handles
/// common args, as well as setting up context for rendering requests
#[derive(Clone, Debug, Parser)]
pub struct BuildRequestCommand {
    /// ID of the recipe to render into a request
    recipe_id: RecipeId,

    /// ID of the profile to pull template values from
    #[clap(long = "profile", short)]
    profile: Option<ProfileId>,

    /// List of key=value template field overrides
    #[clap(
        long = "override",
        short = 'o',
        value_parser = parse_key_val::<String, String>,
    )]
    overrides: Vec<(String, String)>,
}

#[async_trait]
impl Subcommand for RequestCommand {
    async fn execute(self, global: GlobalArgs) -> anyhow::Result<ExitCode> {
        let (database, request) =
            self.build_request.build_request(global).await?;

        if self.dry_run {
            println!("{:#?}", request);
            Ok(ExitCode::SUCCESS)
        } else {
            // Everything other than the body prints to stderr, to make it easy
            // to pipe the body to a file
            if self.headers {
                eprintln!("{}", HeaderDisplay(&request.headers));
            }

            // Run the request
            let config = Config::load()?;
            let http_engine = HttpEngine::new(&config, database);
            let record = http_engine.send(request.into()).await?;
            let status = record.response.status;

            // Print stuff!
            if self.status {
                eprintln!("{}", status.as_u16());
            }
            if self.headers {
                eprintln!("{}", HeaderDisplay(&record.response.headers));
            }
            if !self.no_body {
                // If body is not UTF-8, write the raw bytes instead (e.g if
                // downloading an image)
                let body = &record.response.body;
                if let Some(text) = body.text() {
                    print!("{}", text);
                } else {
                    io::stdout()
                        .write(body.bytes())
                        .context("Error writing to stdout")?;
                }
            }

            if self.exit_status && status.as_u16() >= 400 {
                Ok(ExitCode::from(HTTP_ERROR_EXIT_CODE))
            } else {
                Ok(ExitCode::SUCCESS)
            }
        }
    }
}

impl BuildRequestCommand {
    /// Render the request specified by the user. This returns the DB
    /// connection too so it can be re-used if necessary.
    pub async fn build_request(
        self,
        global: GlobalArgs,
    ) -> anyhow::Result<(CollectionDatabase, Request)> {
        let collection_path = CollectionFile::try_path(global.collection)?;
        let database = Database::load()?.into_collection(&collection_path)?;
        let mut collection_file = CollectionFile::load(collection_path).await?;
        let collection = &mut collection_file.collection;

        // Find profile and recipe by ID
        let profile = self
            .profile
            .map(|profile_id| {
                collection.profiles.swap_remove(&profile_id).ok_or_else(|| {
                    anyhow!(
                        "No profile with ID `{profile_id}`; options are: {}",
                        collection.profiles.keys().join(", ")
                    )
                })
            })
            .transpose()?;

        let recipe = collection
            .recipes
            .swap_remove(&self.recipe_id)
            .ok_or_else(|| {
                anyhow!(
                    "No request with ID `{}`; options are: {}",
                    self.recipe_id,
                    collection.recipes.keys().join(", ")
                )
            })?;

        // Build the request
        let overrides: IndexMap<_, _> = self.overrides.into_iter().collect();
        let template_context = TemplateContext {
            profile,
            chains: collection.chains.clone(),
            database: database.clone(),
            overrides,
            prompter: Box::new(CliPrompter),
        };
        let request = RequestBuilder::new(recipe, RecipeOptions::default())
            .build(&template_context)
            .await?;
        Ok((database, request))
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

/// Wrapper making it easy to print a header map
struct HeaderDisplay<'a>(&'a HeaderMap);

impl<'a> Display for HeaderDisplay<'a> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let key_style = Style::new().bold();
        for (key, value) in self.0 {
            writeln!(
                f,
                "{}: {}",
                key_style.apply_to(key),
                MaybeStr(value.as_bytes()),
            )?;
        }
        Ok(())
    }
}
