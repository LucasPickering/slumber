#![forbid(unsafe_code)]
#![deny(clippy::all)]

//! Command line interface for Slumber.
//!
//! **This crate is not semver compliant**. The version is locked to the root
//! `slumber` crate version. If you choose to depend directly on this crate, you
//! do so at your own risk of breakage.

mod commands;
mod completions;
mod util;

use crate::commands::{
    collections::CollectionsCommand, generate::GenerateCommand,
    history::HistoryCommand, import::ImportCommand, new::NewCommand,
    request::RequestCommand, show::ShowCommand,
};
use clap::{CommandFactory, Parser};
use clap_complete::CompleteEnv;
use std::{path::PathBuf, process::ExitCode};

const COMMAND_NAME: &str = "slumber";

/// Configurable HTTP client with both TUI and CLI interfaces
///
/// If subcommand is omitted, start the TUI.
///
/// https://slumber.lucaspickering.me/book/
#[derive(Debug, Parser)]
#[clap(author, version, about, name = COMMAND_NAME)]
pub struct Args {
    #[command(flatten)]
    pub global: GlobalArgs,
    #[command(subcommand)]
    pub subcommand: Option<CliCommand>,
}

impl Args {
    /// Check if we're in shell completion mode, which is set via the `COMPLETE`
    /// env var. If so, this will print completions then exit the process
    pub fn complete() {
        CompleteEnv::with_factory(Args::command).complete();
    }

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
