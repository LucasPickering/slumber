// One module per subcommand
mod collections;
mod import;
mod request;
mod show;

use crate::{
    cli::{
        collections::CollectionsCommand, import::ImportCommand,
        request::RequestCommand, show::ShowCommand,
    },
    GlobalArgs,
};
use async_trait::async_trait;
use std::process::ExitCode;

/// A CLI subcommand
#[derive(Clone, Debug, clap::Subcommand)]
pub enum CliCommand {
    Request(RequestCommand),
    #[clap(name = "import-experimental")]
    Import(ImportCommand),
    Collections(CollectionsCommand),
    Show(ShowCommand),
}

/// An executable subcommand. This trait isn't strictly necessary because we do
/// static dispatch via the command enum, but it's helpful to enforce a
/// consistent interface for each subcommand.
#[async_trait]
pub trait Subcommand {
    /// Execute the subcommand
    async fn execute(self, global: GlobalArgs) -> anyhow::Result<ExitCode>;
}

impl CliCommand {
    /// Execute a non-TUI command
    pub async fn execute(self, global: GlobalArgs) -> anyhow::Result<ExitCode> {
        match self {
            CliCommand::Request(command) => command.execute(global).await,
            CliCommand::Import(command) => command.execute(global).await,
            CliCommand::Collections(command) => command.execute(global).await,
            CliCommand::Show(command) => command.execute(global).await,
        }
    }
}
