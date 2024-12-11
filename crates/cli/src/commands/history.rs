use crate::{
    completions::{complete_profile, complete_recipe},
    util::HeaderDisplay,
    GlobalArgs, Subcommand,
};
use anyhow::anyhow;
use clap::Parser;
use clap_complete::ArgValueCompleter;
use dialoguer::console::Style;
use slumber_core::{
    collection::{CollectionFile, ProfileId, RecipeId},
    db::{Database, DatabaseMode, ProfileFilter},
    http::{Exchange, ExchangeSummary, RequestId},
    util::{
        format_byte_size, format_duration, format_time, format_time_iso,
        MaybeStr,
    },
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
    },
}

#[derive(Clone, Debug)]
enum RecipeOrRequest {
    Recipe(RecipeId),
    Request(RequestId),
}

impl Subcommand for HistoryCommand {
    async fn execute(self, global: GlobalArgs) -> anyhow::Result<ExitCode> {
        let collection_path = CollectionFile::try_path(None, global.file)?;
        let database = Database::load()?
            .into_collection(&collection_path, DatabaseMode::ReadOnly)?;

        match self.subcommand {
            HistorySubcommand::List { recipe, profile } => {
                let profile_filter = match &profile {
                    None => ProfileFilter::All,
                    Some(None) => ProfileFilter::None,
                    Some(Some(profile_id)) => ProfileFilter::Some(profile_id),
                };
                let exchanges =
                    database.get_all_requests(profile_filter, &recipe)?;
                Self::print_list(exchanges);
            }
            HistorySubcommand::Get { request } => {
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
                Self::print_detail(exchange);
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

    fn print_detail(exchange: Exchange) {
        let header_style = Style::new().bold().underlined();
        let subheader_style = Style::new().bold();

        // Request
        let request = &exchange.request;
        println!("{}", header_style.apply_to("REQUEST"));
        println!("{} {}", subheader_style.apply_to("URL:"), request.url);
        println!("{} {}", subheader_style.apply_to("Method:"), request.method);
        print!(
            "{}\n{}",
            subheader_style.apply_to("Headers"),
            HeaderDisplay(&request.headers)
        );
        if let Some(body) = &request.body {
            print!(
                "{} ({})\n{}",
                subheader_style.apply_to("Body"),
                format_byte_size(body.len()),
                MaybeStr(body)
            )
        }
        println!();

        // Timing
        println!("{}", header_style.apply_to("METADATA"));
        println!(
            "{} {}",
            subheader_style.apply_to("Start Time:"),
            format_time(&exchange.start_time)
        );
        println!(
            "{} {}",
            subheader_style.apply_to("Duration:"),
            format_duration(&exchange.duration())
        );
        println!();

        // Response
        let response = &exchange.response;
        println!("{}", header_style.apply_to("RESPONSE"));
        println!(
            "{} {}",
            subheader_style.apply_to("Status:"),
            response.status
        );
        print!(
            "{}\n{}",
            subheader_style.apply_to("Headers"),
            HeaderDisplay(&response.headers)
        );
        print!(
            "{} ({})\n{}",
            subheader_style.apply_to("Body"),
            format_byte_size(response.body.size()),
            MaybeStr(response.body.bytes())
        );
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
