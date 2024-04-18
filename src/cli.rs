// One module per subcommand
mod collections;
mod generate;
mod import;
mod request;
mod show;

use crate::{
    cli::{
        collections::CollectionsCommand, generate::GenerateCommand,
        import::ImportCommand, request::RequestCommand, show::ShowCommand,
    },
    GlobalArgs,
};
use async_trait::async_trait;
use std::process::ExitCode;

/// A CLI subcommand
#[derive(Clone, Debug, clap::Subcommand)]
pub enum CliCommand {
    Request(RequestCommand),
    Generate(GenerateCommand),
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
            Self::Generate(command) => command.execute(global).await,
            Self::Request(command) => command.execute(global).await,
            Self::Import(command) => command.execute(global).await,
            Self::Collections(command) => command.execute(global).await,
            Self::Show(command) => command.execute(global).await,
        }
    }
}
