use crate::{
    GlobalArgs, Subcommand,
    commands::request::DisplayExchangeCommand,
    completions::{complete_profile, complete_recipe, complete_request_id},
};
use anyhow::{anyhow, bail};
use clap::Parser;
use clap_complete::ArgValueCompleter;
use itertools::Itertools;
use slumber_core::{
    collection::{ProfileId, RecipeId},
    db::{Database, DatabaseMode, ProfileFilter},
    http::RequestId,
    util::{confirm, format_time_iso},
};
use std::{iter, process::ExitCode, str::FromStr};

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

    /// Get a single request/response
    Get {
        /// ID of the request to print. Pass a recipe ID to get the most recent
        /// request for that recipe
        #[clap(
            // Autocomplete recipe IDs, because users won't ever be typing request
            // IDs by hand
            add = ArgValueCompleter::new(complete_recipe),
        )]
        request: RecipeOrRequest,

        #[clap(flatten)]
        display: DisplayExchangeCommand,
    },

    /// Delete requests from history
    ///
    /// The subcommand selects which request(s) to delete. This operation is
    /// irreversible!
    Delete {
        #[clap(subcommand)]
        selection: RequestSelection,

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
                let database = Database::load(DatabaseMode::ReadOnly)?
                    .into_collection(&global.collection_path()?)?;
                let exchanges =
                    database.get_recipe_requests(profile.into(), &recipe)?;
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

            HistorySubcommand::Get { request, display } => {
                let database = Database::load(DatabaseMode::ReadOnly)?
                    .into_collection(&global.collection_path()?)?;
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

            HistorySubcommand::Delete { selection, yes } => {
                if !yes {
                    // Confirmation prompt
                    let prompt = match &selection {
                        RequestSelection::All => {
                            "Delete ALL requests?".to_owned()
                        }
                        RequestSelection::Collection => {
                            let collection_path = global.collection_path()?;
                            format!(
                                "Delete requests for {}?",
                                collection_path.display()
                            )
                        }
                        RequestSelection::Recipe { recipe, profile } => {
                            let profile_label = match profile {
                                None => "all profiles",
                                Some(None) => "no profile",
                                Some(Some(profile_id)) => profile_id,
                            };
                            format!(
                                "Delete requests for recipe `{recipe}` \
                                ({profile_label})?"
                            )
                        }
                        RequestSelection::Request { request } => {
                            format!("Delete request `{}`?", request)
                        }
                    };
                    if !confirm(prompt) {
                        bail!("Cancelled");
                    }
                }

                // Do the deletion
                let database = Database::load(DatabaseMode::ReadWrite)?;
                let deleted = match selection {
                    RequestSelection::All => database.delete_all_requests()?,
                    RequestSelection::Collection => {
                        let database = database
                            .into_collection(&global.collection_path()?)?;
                        database.delete_all_requests()?
                    }
                    RequestSelection::Recipe { recipe, profile } => {
                        let database = database
                            .into_collection(&global.collection_path()?)?;
                        database
                            .delete_recipe_requests(profile.into(), &recipe)?
                    }
                    RequestSelection::Request { request } => {
                        database.delete_request(request)?
                    }
                };
                println!("Deleted {deleted} request(s)");
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

/// An abstraction for a subcommand that supports selecting multiple requests
/// at once.
#[derive(Clone, Debug, Parser)]
enum RequestSelection {
    /// Select all requests across all collections
    All,
    /// Select all requests for the current collection
    Collection,
    /// Select all requests for a recipe in the current collection
    ///
    /// Note: The recipe does not have to currently be in the collection file.
    /// You can view and modify history for recipes that have since been removed
    /// from the collection file.
    Recipe {
        /// Recipe to select requests for
        #[clap(add = ArgValueCompleter::new(complete_recipe))]
        recipe: RecipeId,
        /// Optional filter to select requests for only a single profile. To
        /// select requests that were run under _no_ profile, pass `--profile`
        /// with no value.
        #[clap(
            long = "profile",
            short,
            add = ArgValueCompleter::new(complete_profile),
        )]
        // None -> All profiles
        // Some(None) -> No profile
        // Some(Some("profile1")) -> profile1
        // It'd be nice if we could load directly into ProfileFilter, but I
        // couldn't figure out how to set that up with clap
        profile: Option<Option<ProfileId>>,
    },
    /// Select a single request by ID
    Request {
        #[clap(add = ArgValueCompleter::new(complete_request_id))]
        request: RequestId,
    },
}

/// Print request history as a table
fn print_table<const N: usize>(header: [&str; N], rows: &[[String; N]]) {
    // For each column, find the largest width of any cell
    let mut widths = [0; N];
    for column in 0..N {
        widths[column] = iter::once(header[column].len())
            .chain(rows.iter().map(|row| row[column].len()))
            .max()
            .unwrap_or_default()
            + 1; // Min width, for spacing
    }

    for (header, width) in header.into_iter().zip(widths.iter()) {
        print!("{:<width$}", header, width = width);
    }
    println!();
    for row in rows {
        for (cell, width) in row.iter().zip(widths) {
            print!("{:<width$}", cell, width = width);
        }
        println!();
    }
}
