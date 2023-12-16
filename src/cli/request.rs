use crate::{
    cli::Subcommand,
    collection::{ProfileId, RequestCollection, RequestRecipeId},
    db::Database,
    http::{HttpEngine, RequestBuilder},
    template::{Prompt, Prompter, TemplateContext},
    util::ResultExt,
    GlobalArgs,
};
use anyhow::{anyhow, Context};
use async_trait::async_trait;
use clap::Parser;
use dialoguer::{Input, Password};
use indexmap::IndexMap;
use itertools::Itertools;
use std::{error::Error, str::FromStr};

/// Execute a single request
#[derive(Clone, Debug, Parser)]
#[clap(aliases=&["req", "rq"])]
pub struct RequestCommand {
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
}

#[async_trait]
impl Subcommand for RequestCommand {
    async fn execute(self, global: GlobalArgs) -> anyhow::Result<()> {
        let collection_path = RequestCollection::try_path(global.collection)?;
        let database = Database::load()?.into_collection(&collection_path)?;
        let mut collection = RequestCollection::load(collection_path).await?;

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
            .swap_remove(&self.request_id)
            .ok_or_else(|| {
                anyhow!(
                    "No request with ID `{}`; options are: {}",
                    self.request_id,
                    collection.recipes.keys().join(", ")
                )
            })?;

        // Build the request
        let overrides: IndexMap<_, _> = self.overrides.into_iter().collect();
        let request = RequestBuilder::new(
            recipe,
            TemplateContext {
                profile,
                chains: collection.chains.clone(),
                database: database.clone(),
                overrides,
                prompter: Box::new(CliPrompter),
            },
        )
        .build()
        .await?;

        if self.dry_run {
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
