use crate::{
    GlobalArgs, Subcommand,
    commands::request::DisplayExchangeCommand,
    completions::{complete_profile, complete_recipe, complete_request_id},
    util::print_table,
};
use anyhow::{anyhow, bail};
use chrono::{
    DateTime, Local, Utc,
    format::{DelayedFormat, StrftimeItems},
};
use clap::Parser;
use itertools::Itertools;
use slumber_core::{
    collection::{ProfileId, RecipeId},
    database::{Database, ProfileFilter},
    http::RequestId,
};
use std::{process::ExitCode, str::FromStr};

/// View and modify request history
///
/// Requests made in the Slumber TUI are stored in a local database so past
/// requests can be viewed in the TUI. This subcommand allows you to browse and
/// prune that history.
#[derive(Clone, Debug, Parser)]
pub struct HistoryCommand {
    #[command(subcommand)]
    subcommand: HistorySubcommand,
}

#[derive(Clone, Debug, clap::Subcommand)]
enum HistorySubcommand {
    /// List requests
    #[command(visible_alias = "ls")]
    List {
        /// Recipe to show requests for. Omit to show requests for all recipes
        #[clap(add = complete_recipe())]
        recipe: Option<RecipeId>,

        /// Only show requests for a single profile. To show requests that were
        /// run under _no_ profile, pass `--profile` with no value. This must
        /// be used in conjunction with a recipe ID.
        #[clap(long = "profile", short, add = complete_profile())]
        // None -> All profiles
        // Some(None) -> No profile
        // Some(Some("profile1")) -> profile1
        #[expect(clippy::option_option)]
        profile: Option<Option<ProfileId>>,

        /// Show requests for all collections, not just the current
        #[clap(short, long)]
        all: bool,

        /// Print only request IDs, with no header
        #[clap(long)]
        id_only: bool,
    },

    /// Get a single request/response
    Get {
        /// ID of the request to print. Pass a recipe ID to get the most recent
        /// request for that recipe
        #[clap(
            // Autocomplete recipe IDs, because users won't ever be typing request
            // IDs by hand
            add = complete_recipe(),
        )]
        request: RecipeOrRequest,

        #[clap(flatten)]
        display: DisplayExchangeCommand,
    },

    /// Delete requests from history
    ///
    /// This operation is irreversible! Combine with `slumber history list
    /// --id-only` to delete requests in bulk. To delete all requests for a
    /// collection, you can also use `slumber collections delete`
    #[command(visible_alias = "rm")]
    Delete {
        /// Request ID(s) to delete
        #[clap(num_args = 1..)]
        request: Vec<RequestId>,
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
            HistorySubcommand::List {
                recipe,
                profile,
                all,
                id_only,
            } => {
                let database = Database::load()?;
                let exchanges = match (recipe, profile, all) {
                    // All requests for all collections
                    (None, None, true) => database.get_all_requests()?,
                    // All requests for the current collection
                    (None, None, false) => database
                        .into_collection(&global.collection_file()?)?
                        .get_all_requests()?,
                    // All requests for a single recipe in current collection
                    (Some(recipe_id), profile, false) => database
                        .into_collection(&global.collection_file()?)?
                        .get_recipe_requests(profile.into(), &recipe_id)?,

                    // Reject invalid arg groupings. This is a bit of a code
                    // stink because invalid states should generally be
                    // unrepresentable, but using a more rigid schema like the
                    // `delete` subcommand makes the whole thing clunkier
                    (Some(_), _, true) => {
                        bail!("Cannot specify `--all` with a recipe")
                    }
                    (None, Some(_), _) => {
                        bail!("Cannot specify `--profile` without a recipe")
                    }
                };

                if id_only {
                    for exchange in exchanges {
                        println!("{}", exchange.id);
                    }
                } else {
                    print_table(
                        ["Recipe", "Profile", "Time", "Status", "Request ID"],
                        &exchanges
                            .into_iter()
                            .map(|exchange| {
                                [
                                    exchange.recipe_id.to_string(),
                                    exchange
                                        .profile_id
                                        .map(ProfileId::into)
                                        .unwrap_or_default(),
                                    format_time_iso(&exchange.start_time)
                                        .to_string(),
                                    exchange.status.as_u16().to_string(),
                                    exchange.id.to_string(),
                                ]
                            })
                            .collect_vec(),
                    );
                }
            }

            HistorySubcommand::Get { request, display } => {
                let database = Database::load()?
                    .into_collection(&global.collection_file()?)?;
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

            HistorySubcommand::Delete { request } => {
                // Do the deletion
                let database = Database::load()?;
                for id in &request {
                    database.delete_request(*id)?;
                }
                println!("Deleted {} request(s)", request.len());
            }
        }
        Ok(ExitCode::SUCCESS)
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

/// An abstraction for selecting multiple requests for deletion. We use this
/// for deletion, while the arg grouping is simpler for listing, for a few
/// reasons:
/// - Deletion is destructive, so we want the selection to be more explicit
/// - Deletion also supports a single request by ID, which would be pretty
///   useless for listing
#[derive(Clone, Debug, Parser)]
enum DeleteSelection {
    /// Delete all requests across all collections
    All,
    /// Delete all requests for the current collection
    Collection,
    /// Delete all requests for a recipe in the current collection
    ///
    /// Note: The recipe does not have to currently be in the collection file.
    /// You can view and modify history for recipes that have since been removed
    /// from the collection file.
    Recipe {
        /// Recipe to delete requests for
        #[clap(add = complete_recipe())]
        recipe: RecipeId,
        /// Optional filter to delete requests for only a single profile. To
        /// delete requests that _no_ associated profile, pass `--profile`
        /// with no value.
        #[clap(
            long = "profile",
            short,
            add = complete_profile(),
        )]
        // None -> All profiles
        // Some(None) -> No profile
        // Some(Some("profile1")) -> profile1
        // It'd be nice if we could load directly into ProfileFilter, but I
        // couldn't figure out how to set that up with clap
        #[expect(clippy::option_option)]
        profile: Option<Option<ProfileId>>,
    },
    /// Delete a single request by ID
    Request {
        #[clap(add = complete_request_id())]
        request: RequestId,
    },
}

/// Format a datetime in ISO 8601 format
fn format_time_iso(time: &DateTime<Utc>) -> DelayedFormat<StrftimeItems> {
    time.with_timezone(&Local).format("%FT%TZ%Z")
}
