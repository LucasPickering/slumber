//! Command line interface for Slumber.
//!
//! **This crate is not semver compliant**. The version is locked to the root
//! `slumber` crate version. If you choose to depend directly on this crate, you
//! do so at your own risk of breakage.

mod commands;
mod completions;
mod util;

pub use util::print_error;

use crate::{
    commands::{
        collection::CollectionCommand, config::ConfigCommand, db::DbCommand,
        generate::GenerateCommand, import::ImportCommand, new::NewCommand,
        request::RequestCommand,
    },
    completions::complete_collection_path,
};
use clap::{CommandFactory, Parser};
use clap_complete::CompleteEnv;
use slumber_core::collection::{CollectionError, CollectionFile};
use slumber_util::paths;
use std::{path::PathBuf, process::ExitCode};

const COMMAND_NAME: &str = "slumber";

/// Configurable HTTP client with both TUI and CLI interfaces
///
/// If subcommand is omitted, start the TUI.
///
/// https://slumber.lucaspickering.me/
#[derive(Debug, Parser)]
#[clap(author, version, about, name = COMMAND_NAME)]
#[expect(rustdoc::bare_urls)]
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
#[derive(Debug, Default, Parser)]
pub struct GlobalArgs {
    /// Collection file, which defines profiles, recipes, etc.
    ///
    /// If omitted, check the current and all parent directories for the
    /// following files (in this order): slumber.yml, slumber.yaml,
    /// .slumber.yml, .slumber.yaml. If a directory is passed, apply the
    /// same search logic from the given directory rather than the current.
    #[clap(long, short, add = complete_collection_path())]
    pub file: Option<PathBuf>,

    /// Print the path to the log file for this session
    #[clap(long)]
    pub print_log_path: bool,

    /// Print logs to stdout during CLI operation
    ///
    /// Specify multiple times to increase verbosity. Supports up to -vvv. Has
    /// no effect in TUI mode.
    #[clap(short, long, action = clap::ArgAction::Count)]
    pub verbose: u8,
}

impl GlobalArgs {
    /// Get the path to the active collection file. Return an error if there is
    /// no collection file present, or if the user specified an invalid file.
    fn collection_file(&self) -> Result<CollectionFile, CollectionError> {
        CollectionFile::new(self.file.clone())
    }
}

/// A CLI subcommand
#[derive(Clone, Debug, clap::Subcommand)]
pub enum CliCommand {
    Collection(CollectionCommand),
    Config(ConfigCommand),
    Db(DbCommand),
    Generate(GenerateCommand),
    Import(ImportCommand),
    New(NewCommand),
    Request(RequestCommand),
}

impl CliCommand {
    /// Execute this CLI subcommand
    pub async fn execute(self, global: GlobalArgs) -> anyhow::Result<ExitCode> {
        if global.print_log_path {
            let path = paths::log_file();
            println!("Logging to {}", path.display());
        }

        match self {
            Self::Collection(command) => command.execute(global).await,
            Self::Config(command) => command.execute(global).await,
            Self::Db(command) => command.execute(global).await,
            Self::Generate(command) => command.execute(global).await,
            Self::Import(command) => command.execute(global).await,
            Self::New(command) => command.execute(global).await,
            Self::Request(command) => command.execute(global).await,
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
