//! Command line interface for Slumber.
//!
//! **This crate is not semver compliant**. The version is locked to the root
//! `slumber` crate version. If you choose to depend directly on this crate, you
//! do so at your own risk of breakage.

mod commands;
mod completions;
mod util;

pub use util::print_error;

#[cfg(feature = "import")]
use crate::commands::import::ImportCommand;
use crate::{
    commands::{
        collection::CollectionCommand, config::ConfigCommand, db::DbCommand,
        generate::GenerateCommand, new::NewCommand, request::RequestCommand,
    },
    completions::{complete_collection_path, complete_log_level},
};
use clap::{CommandFactory, Parser};
use clap_complete::CompleteEnv;
use slumber_core::collection::{CollectionError, CollectionFile};
use slumber_util::paths;
use std::{path::PathBuf, process::ExitCode};
use tracing::level_filters::LevelFilter;

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
#[derive(Debug, Parser)]
pub struct GlobalArgs {
    /// Collection file, which defines profiles, recipes, etc.
    ///
    /// If omitted, check the current and all parent directories for the
    /// following files (in this order): slumber.yml, slumber.yaml,
    /// .slumber.yml, .slumber.yaml. If a directory is passed, apply the
    /// same search logic from the given directory rather than the current.
    #[clap(long, short, add = complete_collection_path())]
    pub file: Option<PathBuf>,

    /// Set logging verbosity
    ///
    /// For the CLI, this will set the verbosity of stderr output, which
    /// defaults to off. For both the CLI and the TUI, this will also set the
    /// verbosity of file logging. HOWEVER, file logging can never be below
    /// `warn`. Therefore, `--log-level off` has no impact on file logging.
    ///
    /// Available options (in increasing verbosity) are:
    /// - off
    /// - error
    /// - warn
    /// - info
    /// - debug
    /// - trace
    #[clap(long, default_value_t = LevelFilter::OFF, add = complete_log_level())]
    pub log_level: LevelFilter,

    /// Print the path to the log file for this session
    #[clap(long)]
    pub print_log_path: bool,

    /// Test only: set the directory for the config, database, and log files
    #[cfg(debug_assertions)]
    #[clap(long, hide = true)]
    pub data_dir: Option<PathBuf>,
}

impl GlobalArgs {
    /// Get the path to the active collection file. Return an error if there is
    /// no collection file present, or if the user specified an invalid file.
    fn collection_file(&self) -> Result<CollectionFile, CollectionError> {
        CollectionFile::new(self.file.clone())
    }
}

impl Default for GlobalArgs {
    fn default() -> Self {
        Self {
            file: None,
            log_level: LevelFilter::OFF,
            print_log_path: false,
            #[cfg(debug_assertions)]
            data_dir: None,
        }
    }
}

/// A CLI subcommand
#[derive(Clone, Debug, clap::Subcommand)]
pub enum CliCommand {
    Collection(CollectionCommand),
    Config(ConfigCommand),
    Db(DbCommand),
    Generate(GenerateCommand),
    #[cfg(feature = "import")]
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

        // The --data-dir flag is used in integration tests to isolate files
        #[cfg(debug_assertions)]
        if let Some(path) = global.data_dir.as_deref() {
            paths::set_data_directory(path.to_owned());
        }

        match self {
            Self::Collection(command) => command.execute(global).await,
            Self::Config(command) => command.execute(global).await,
            Self::Db(command) => command.execute(global).await,
            Self::Generate(command) => command.execute(global).await,
            #[cfg(feature = "import")]
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
