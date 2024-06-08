use crate::{
    cli::Subcommand,
    collection::{CollectionFile, ProfileId, RecipeId},
    db::Database,
    http::{Exchange, ExchangeSummary, RequestId},
    util::{format_duration, format_time, HeaderDisplay, MaybeStr},
    GlobalArgs,
};
use anyhow::anyhow;
use bytesize::ByteSize;
use clap::Parser;
use dialoguer::console::Style;
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
        recipe: RecipeId,

        /// Profile to query for. If omitted, query for requests with no
        /// profile
        #[clap(long = "profile", short)]
        profile: Option<ProfileId>,
    },

    /// Print an entire request/response by ID
    Get { request: RequestId },
}

impl Subcommand for HistoryCommand {
    async fn execute(self, global: GlobalArgs) -> anyhow::Result<ExitCode> {
        warn!(
            "`history` command is unstable; \
            it may change or be removed at any time"
        );
        let collection_path = CollectionFile::try_path(None, global.file)?;
        let database = Database::load()?.into_collection(&collection_path)?;

        match self.subcommand {
            HistorySubcommand::List { recipe, profile } => {
                let exchanges =
                    database.get_all_requests(profile.as_ref(), &recipe)?;
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
                "{} {} {}",
                exchange.id,
                exchange.status,
                format_time(&exchange.start_time)
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
                ByteSize(body.len() as u64),
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
            response.body.size(),
            MaybeStr(response.body.bytes())
        );
    }
}
