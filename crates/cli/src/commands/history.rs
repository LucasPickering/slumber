use crate::{
    GlobalArgs, Subcommand,
    commands::request::DisplayExchangeCommand,
    completions::{complete_profile, complete_recipe},
};
use anyhow::{anyhow, bail};
use clap::Parser;
use clap_complete::ArgValueCompleter;
use slumber_core::{
    collection::{ProfileId, RecipeId},
    db::{Database, DatabaseMode, ProfileFilter},
    http::{ExchangeSummary, RequestId},
    util::{confirm, format_time_iso},
};
use std::{process::ExitCode, str::FromStr};

/// View request history
#[derive(Clone, Debug, Parser)]
pub struct HistoryCommand {
    #[command(subcommand)]
    subcommand: HistorySubcommand,
}

#[derive(Clone, Debug, clap::Subcommand)]
enum HistorySubcommand {
    /// List all requests for a recipe
    #[command(visible_alias = "ls")]
    List {
        /// Recipe to show requests for
        #[clap(add = ArgValueCompleter::new(complete_recipe))]
        recipe: RecipeId,

        /// Only show recipes for a single profile. If this argument is passed
        /// with no value, requests with no associated profile are shown
        #[clap(
            long = "profile",
            short,
            add = ArgValueCompleter::new(complete_profile),
        )]
        // None -> All profiles
        // Some(None) -> No profile
        // Some(Some("profile1")) -> profile1
        profile: Option<Option<ProfileId>>,
    },

    /// Print an entire request/response
    Get {
        /// ID of the request to print. Pass a recipe ID to get the most recent
        /// request for that recipe
        // Autocomplete recipe IDs, because users won't ever be typing request
        // IDs by hand
        #[clap(add = ArgValueCompleter::new(complete_recipe))]
        request: RecipeOrRequest,

        #[clap(flatten)]
        display: DisplayExchangeCommand,
    },

    /// Delete a single request, or all requests for a single recipe
    Delete {
        /// ID of the request or recipe to delete
        ///
        /// Pass a request ID to delete a single request, or a recipe ID to
        /// delete all requests for that recipe
        #[clap(add = ArgValueCompleter::new(complete_recipe))]
        request: RecipeOrRequest,

        /// Only delete recipes for a single profile. If this argument is
        /// passed with no value, requests with no associated profile are
        /// deleted
        #[clap(
            long = "profile",
            short,
            add = ArgValueCompleter::new(complete_profile),
        )]
        // None -> All profiles
        // Some(None) -> No profile
        // Some(Some("profile1")) -> profile1
        profile: Option<Option<ProfileId>>,
    },

    /// Delete ALL request history for the current collection
    ///
    /// This is a dangerous and irreversible operation!
    Clear {
        /// Delete all request history for ALL collections
        #[clap(long)]
        all: bool,
        /// Skip the confirmation prompt
        #[clap(long, short)]
        yes: bool,
    },
}

#[derive(Clone, Debug)]
enum RecipeOrRequest {
    Recipe(RecipeId),
    Request(RequestId),
}

impl Subcommand for HistoryCommand {
    async fn execute(self, global: GlobalArgs) -> anyhow::Result<ExitCode> {
        match self.subcommand {
            HistorySubcommand::List { recipe, profile } => {
                let database = Database::load()?.into_collection(
                    &global.collection_path()?,
                    DatabaseMode::ReadOnly,
                )?;
                let exchanges = database
                    .get_recipe_requests(profile.as_ref().into(), &recipe)?;
                Self::print_list(exchanges);
            }

            HistorySubcommand::Get { request, display } => {
                let database = Database::load()?.into_collection(
                    &global.collection_path()?,
                    DatabaseMode::ReadOnly,
                )?;
                let exchange = match request {
                    RecipeOrRequest::Recipe(recipe_id) => database
                        .get_latest_request(ProfileFilter::All, &recipe_id)?
                        .ok_or_else(|| {
                            anyhow!("Recipe `{recipe_id}` has no history")
                        })?,
                    RecipeOrRequest::Request(request_id) => {
                        database.get_request(request_id)?.ok_or_else(|| {
                            anyhow!("Request `{request_id}` not found")
                        })?
                    }
                };
                display.write_request(&exchange.request);
                display.write_response(&exchange.response)?;
            }

            HistorySubcommand::Delete { request, profile } => {
                let database = Database::load()?.into_collection(
                    &global.collection_path()?,
                    DatabaseMode::ReadWrite,
                )?;
                match request {
                    RecipeOrRequest::Recipe(recipe_id) => {
                        let deleted = database.delete_recipe_requests(
                            profile.as_ref().into(),
                            &recipe_id,
                        )?;
                        println!("Deleted {deleted} request(s)");
                    }
                    RecipeOrRequest::Request(request_id) => {
                        let deleted = database.delete_request(request_id)?;
                        println!("Deleted {deleted} request(s)");
                    }
                }
            }

            HistorySubcommand::Clear { all, yes } => {
                let database = Database::load()?;
                let deleted = if all {
                    // Delete history for all collections. This should be
                    // callable even when a collection file isn't present
                    if !yes
                        && !confirm(
                            "Delete request history for ALL collections?",
                        )
                    {
                        bail!("Cancelled");
                    }
                    database.delete_all_requests()?
                } else {
                    // Delete just for the current collection
                    let collection_path = global.collection_path()?;
                    if !yes
                        && !confirm(format!(
                            "Delete request history for {}?",
                            collection_path.display()
                        ))
                    {
                        bail!("Cancelled");
                    }
                    let database = database.into_collection(
                        &collection_path,
                        DatabaseMode::ReadWrite,
                    )?;
                    database.delete_all_requests()?
                };
                println!("Deleted {deleted} request(s)");
            }
        }
        Ok(ExitCode::SUCCESS)
    }
}

impl HistoryCommand {
    fn print_list(exchanges: Vec<ExchangeSummary>) {
        for exchange in exchanges {
            println!(
                "{}\t{}\t{}\t{}",
                exchange.profile_id.as_deref().unwrap_or_default(),
                exchange.id,
                exchange.status.as_str(),
                format_time_iso(&exchange.start_time),
            );
        }
    }
}

impl FromStr for RecipeOrRequest {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(s.parse::<RequestId>()
            .map(Self::Request)
            .unwrap_or_else(|_| {
                RecipeOrRequest::Recipe(RecipeId::from(s.to_owned()))
            }))
    }
}
