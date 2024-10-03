#![forbid(unsafe_code)]
#![deny(clippy::all)]

//! Command line interface for Slumber.
//!
//! **This crate is not semver compliant**. The version is locked to the root
//! `slumber` crate version. If you choose to depend directly on this crate, you
//! do so at your own risk of breakage.

mod commands;
mod util;

use crate::commands::{
    collections::CollectionsCommand, completions::CompletionsCommand,
    generate::GenerateCommand, history::HistoryCommand, import::ImportCommand,
    new::NewCommand, request::RequestCommand, show::ShowCommand,
};
use clap::Parser;
use std::{path::PathBuf, process::ExitCode};

const COMMAND_NAME: &str = "slumber";

#[derive(Debug, Parser)]
#[clap(
    author,
    version,
    about,
    name = COMMAND_NAME,
    long_about = "Configurable HTTP client with both TUI and CLI interfaces"
)]
pub struct Args {
    #[command(flatten)]
    pub global: GlobalArgs,
    /// Subcommand to execute. If omitted, run the TUI
    #[command(subcommand)]
    pub subcommand: Option<CliCommand>,
}

impl Args {
    /// Alias for [clap::Parser::parse]
    pub fn parse() -> Self {
        <Self as Parser>::parse()
    }
}

/// Arguments that are available to all subcommands and the TUI
#[derive(Debug, Parser)]
pub struct GlobalArgs {
    /// Collection file, which defines profiles, recipes, etc. If omitted,
    /// check the current and all parent directories for the following files
    /// (in this order): slumber.yml, slumber.yaml, .slumber.yml, .slumber.yaml
    #[clap(long, short)]
    pub file: Option<PathBuf>,
}

/// A CLI subcommand
#[derive(Clone, Debug, clap::Subcommand)]
pub enum CliCommand {
    Collections(CollectionsCommand),
    Completions(CompletionsCommand),
    Generate(GenerateCommand),
    History(HistoryCommand),
    Import(ImportCommand),
    New(NewCommand),
    Request(RequestCommand),
    Show(ShowCommand),
}

impl CliCommand {
    /// Execute this CLI subcommand
    pub async fn execute(self, global: GlobalArgs) -> anyhow::Result<ExitCode> {
        match self {
            Self::Collections(command) => command.execute(global).await,
            Self::Completions(command) => command.execute(global).await,
            Self::Generate(command) => command.execute(global).await,
            Self::History(command) => command.execute(global).await,
            Self::Import(command) => command.execute(global).await,
            Self::New(command) => command.execute(global).await,
            Self::Request(command) => command.execute(global).await,
            Self::Show(command) => command.execute(global).await,
        }
    }
}

/// An executable subcommand. This trait isn't strictly necessary because we do
/// static dispatch via the command enum, but it's helpful to enforce a
/// consistent interface for each subcommand.
trait Subcommand {
    /// Execute the subcommand
    async fn execute(self, global: GlobalArgs) -> anyhow::Result<ExitCode>;
}
