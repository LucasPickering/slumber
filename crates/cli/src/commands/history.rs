use crate::{
    completions::{complete_profile, complete_recipe},
    util::HeaderDisplay,
    GlobalArgs, Subcommand,
};
use anyhow::anyhow;
use clap::{Parser, ValueHint};
use clap_complete::ArgValueCompleter;
use dialoguer::console::Style;
use slumber_core::{
    collection::{CollectionFile, ProfileId, RecipeId},
    db::{Database, DatabaseMode},
    http::{Exchange, ExchangeSummary, RequestId},
    util::{
        format_byte_size, format_duration, format_time, format_time_iso,
        MaybeStr,
    },
};
use std::process::ExitCode;
use tracing::warn;

/// View request collection history (unstable)
#[derive(Clone, Debug, Parser)]
#[command(hide = true)] // Hidden because unstable
pub struct HistoryCommand {
    #[command(subcommand)]
    subcommand: HistorySubcommand,
}

#[derive(Clone, Debug, clap::Subcommand)]
enum HistorySubcommand {
    /// List all requests for a recipe/profile combination
    #[command(visible_alias = "ls")]
    List {
        /// Recipe to query for
        #[clap(add = ArgValueCompleter::new(complete_recipe))]
        recipe: RecipeId,

        /// Profile to query for. If omitted, show requests for all profiles.
        /// Pass --profile with no value to show requests with no profile.
        #[clap(
            long = "profile",
            short,
            add = ArgValueCompleter::new(complete_profile),
        )]
        profile: Option<Option<ProfileId>>,
    },

    /// Print an entire request/response
    Get {
        // Disable completion for this arg. We could load all the request IDs
        // from the DB, but that's not worth the effort since this is an
        // unstable command still and people will rarely be typing an ID by
        // hand, they'll typically just copy paste
        /// ID of the request/response to print
        #[clap(value_hint = ValueHint::Other)]
        request: RequestId,
    },
}

impl Subcommand for HistoryCommand {
    async fn execute(self, global: GlobalArgs) -> anyhow::Result<ExitCode> {
        warn!(
            "`history` command is unstable; \
            it may change or be removed at any time"
        );
        let collection_path = CollectionFile::try_path(None, global.file)?;
        let database = Database::load()?
            .into_collection(&collection_path, DatabaseMode::ReadOnly)?;

        match self.subcommand {
            HistorySubcommand::List { recipe, profile } => {
                let exchanges = if let Some(profile) = profile.as_ref() {
                    database.get_profile_requests(profile.as_ref(), &recipe)?
                } else {
                    database.get_all_requests(&recipe)?
                };
                Self::print_list(exchanges);
            }
            HistorySubcommand::Get { request } => {
                let exchange = database
                    .get_request(request)?
                    .ok_or_else(|| anyhow!("Request `{request}` not found"))?;
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
