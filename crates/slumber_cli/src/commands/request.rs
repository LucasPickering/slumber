use crate::{util::HeaderDisplay, GlobalArgs, Subcommand};
use anyhow::{anyhow, Context};
use clap::Parser;
use dialoguer::{Input, Password, Select as DialoguerSelect};
use indexmap::IndexMap;
use itertools::Itertools;
use slumber_config::Config;
use slumber_core::{
    collection::{CollectionFile, ProfileId, RecipeId},
    db::{CollectionDatabase, Database},
    http::{BuildOptions, HttpEngine, RequestSeed, RequestTicket},
    template::{Prompt, Prompter, Select, TemplateContext, TemplateError},
    util::ResultTraced,
};
use std::{
    error::Error,
    io::{self, Write},
    process::ExitCode,
    str::FromStr,
};
use tracing::warn;

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

    /// Just print the generated request, instead of sending it. Triggered
    /// sub-requests will also not be executed.
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

impl Subcommand for RequestCommand {
    async fn execute(self, global: GlobalArgs) -> anyhow::Result<ExitCode> {
        let (database, ticket) = self
            .build_request
            // Don't execute sub-requests in a dry run
            .build_request(global, !self.dry_run)
            .await
            .map_err(|error| {
                // If the build failed because triggered requests are disabled,
                // replace it with a custom error message
                if TemplateError::has_trigger_disabled_error(&error) {
                    error.context(
                        "Triggered requests are disabled with `--dry-run`",
                    )
                } else {
                    error
                }
            })?;

        if self.dry_run {
            println!("{:#?}", ticket.record());
            Ok(ExitCode::SUCCESS)
        } else {
            // Everything other than the body prints to stderr, to make it easy
            // to pipe the body to a file
            if self.headers {
                eprintln!("{}", HeaderDisplay(&ticket.record().headers));
            }

            // Run the request
            let exchange = ticket.send(&database).await?;
            let status = exchange.response.status;

            // Print stuff!
            if self.status {
                eprintln!("{}", status.as_u16());
            }
            if self.headers {
                eprintln!("{}", HeaderDisplay(&exchange.response.headers));
            }
            if !self.no_body {
                // If body is not UTF-8, write the raw bytes instead (e.g if
                // downloading an image)
                let body = &exchange.response.body;
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
    /// Render the request specified by the user. This returns the HTTP engine
    /// too so it can be re-used if necessary (iff `trigger_dependencies` is
    /// enabled).
    ///
    /// `trigger_dependencies` controls whether chained requests can be executed
    /// if their triggers apply.
    pub async fn build_request(
        self,
        global: GlobalArgs,
        trigger_dependencies: bool,
    ) -> anyhow::Result<(CollectionDatabase, RequestTicket)> {
        let collection_path = CollectionFile::try_path(None, global.file)?;
        let database = Database::load()?.into_collection(&collection_path)?;
        let collection_file = CollectionFile::load(collection_path).await?;
        let collection = collection_file.collection;
        let config = Config::load()?;
        let http_engine = HttpEngine::new(&config.http);

        // Validate profile ID, so we can provide a good error if it's invalid
        if let Some(profile_id) = &self.profile {
            collection.profiles.get(profile_id).ok_or_else(|| {
                anyhow!(
                    "No profile with ID `{profile_id}`; options are: {}",
                    collection.profiles.keys().format(", ")
                )
            })?;
        }

        // Build the request
        let overrides: IndexMap<_, _> = self.overrides.into_iter().collect();
        let template_context = TemplateContext {
            selected_profile: self.profile.clone(),
            collection,
            // Passing the HTTP engine is how we tell the template renderer that
            // it's ok to execute subrequests during render
            http_engine: if trigger_dependencies {
                Some(http_engine.clone())
            } else {
                None
            },
            database: database.clone(),
            overrides,
            prompter: Box::new(CliPrompter),
            state: Default::default(),
        };
        let seed = RequestSeed::new(self.recipe_id, BuildOptions::default());
        let request = http_engine.build(seed, &template_context).await?;
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
        let result = if prompt.sensitive {
            // Dialoguer doesn't support default values here so there's nothing
            // we can do
            if prompt.default.is_some() {
                warn!(
                    "Default value not supported for sensitive prompts in CLI"
                );
            }

            Password::new()
                .with_prompt(prompt.message)
                .allow_empty_password(true)
                .interact()
        } else {
            let mut input =
                Input::new().with_prompt(prompt.message).allow_empty(true);
            if let Some(default) = prompt.default {
                input = input.default(default);
            }
            input.interact()
        };

        // If we failed to read the value, print an error and report nothing
        if let Ok(value) =
            result.context("Error reading value from prompt").traced()
        {
            prompt.channel.respond(value);
        }
    }

    fn select(&self, select: Select) {
        let result = DialoguerSelect::new()
            .with_prompt(select.message)
            .items(&select.options)
            .interact();

        // If we failed to read the value, print an error and report nothing
        if let Ok(value) =
            result.context("Error reading value from select").traced()
        {
            select.channel.respond(select.options[value].clone());
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
