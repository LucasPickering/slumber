use crate::{GlobalArgs, Subcommand};
use clap::Parser;
use serde::Serialize;
use slumber_config::Config;
use slumber_core::{database::Database, util};
use slumber_util::paths;
use std::{path::Path, process::ExitCode};

/// Print meta information about Slumber (config, collections, etc.)
#[derive(Clone, Debug, Parser)]
pub struct ShowCommand {
    #[command(subcommand)]
    target: ShowTarget,
}

#[derive(Copy, Clone, Debug, clap::Subcommand)]
enum ShowTarget {
    /// Print the path of directories/files that Slumber uses
    Paths {
        /// Print the path for just a single target
        target: Option<PathsTarget>,
    },
    /// Print global Slumber configuration
    ///
    /// This loads the config and re-stringifies it, so it will print exactly
    /// what Slumber will use in action.
    Config {
        /// Open the configuration file in the default editor
        ///
        /// See docs for how you can configure which editor to use:
        /// https://slumber.lucaspickering.me/user_guide/tui/editor.html#editing
        #[clap(long)]
        #[expect(rustdoc::bare_urls)]
        edit: bool,
    },
    /// Print current request collection
    ///
    /// This loads the collection and re-stringifies it, so it will print
    /// exactly what Slumber will use in action.
    Collection {
        /// Open the configuration file in the default editor
        ///
        /// See docs for how you can configure which editor to use:
        /// https://slumber.lucaspickering.me/user_guide/tui/editor.html#editing
        #[clap(long)]
        #[expect(rustdoc::bare_urls)]
        edit: bool,
    },
}

#[derive(Copy, Clone, Debug, clap::ValueEnum)]
enum PathsTarget {
    Collection,
    Config,
    #[value(name = "db")]
    Database,
    Log,
}

impl Subcommand for ShowCommand {
    async fn execute(self, global: GlobalArgs) -> anyhow::Result<ExitCode> {
        match self.target {
            // Print paths
            ShowTarget::Paths { target: None } => {
                println!("Config: {}", Config::path().display());
                println!("Database: {}", Database::path().display());
                println!("Log file: {}", paths::log_file().display());
                println!(
                    "Collection: {}",
                    global
                        .collection_file()
                        .map(|file| file.to_string())
                        .unwrap_or_else(|error| error.to_string())
                );
                Ok(ExitCode::SUCCESS)
            }
            ShowTarget::Paths {
                target: Some(PathsTarget::Config),
            } => {
                println!("{}", Config::path().display());
                Ok(ExitCode::SUCCESS)
            }
            ShowTarget::Paths {
                target: Some(PathsTarget::Collection),
            } => {
                println!("{}", global.collection_file()?);
                Ok(ExitCode::SUCCESS)
            }
            ShowTarget::Paths {
                target: Some(PathsTarget::Database),
            } => {
                println!("{}", Database::path().display());
                Ok(ExitCode::SUCCESS)
            }
            ShowTarget::Paths {
                target: Some(PathsTarget::Log),
            } => {
                println!("{}", paths::log_file().display());
                Ok(ExitCode::SUCCESS)
            }

            // Print config
            ShowTarget::Config { edit } => {
                if edit {
                    // If the config is invalid, the user is probably trying to
                    // fix it so we should open anyway
                    let config = Config::load().unwrap_or_default();
                    let path = Config::path();
                    edit_and_validate(&config, &path, || Config::load().is_ok())
                } else {
                    let config = Config::load()?;
                    println!("{}", to_yaml(&config));
                    Ok(ExitCode::SUCCESS)
                }
            }
            // Print collection
            ShowTarget::Collection { edit } => {
                let collection_file = global.collection_file()?;
                if edit {
                    let config = Config::load()?;
                    edit_and_validate(&config, collection_file.path(), || {
                        collection_file.load().is_ok()
                    })
                } else {
                    let collection = collection_file.load()?;
                    println!("{}", to_yaml(&collection));
                    Ok(ExitCode::SUCCESS)
                }
            }
        }
    }
}

fn to_yaml<T: Serialize>(value: &T) -> String {
    // Panic is intentional, indicates a wonky bug
    serde_yaml::to_string(value).expect("Error serializing")
}

/// Open a file in the user's configured editor. After the user closes the
/// editor, check if the file is valid using the given predicate. If it's
/// invalid, let the user know and offer to reopen it. This loop will repeat
/// indefinitely until the file is valid or the user chooses to exit.
fn edit_and_validate(
    config: &Config,
    path: &Path,
    is_valid: impl Fn() -> bool,
) -> anyhow::Result<ExitCode> {
    loop {
        let mut command = config.tui.editor_command(path)?;
        let status = command.spawn()?.wait()?;

        // After editing, verify the file is valid. If not, offer to reopen
        if !is_valid()
            && util::confirm(format!(
                "{path} is invalid, would you like to reopen it?",
                path = path.display(),
            ))
        {
            continue;
        }

        // https://doc.rust-lang.org/stable/std/process/struct.ExitStatus.html#differences-from-exitcode
        let code = status.code().and_then(|code| u8::try_from(code).ok());
        return Ok(code.map(ExitCode::from).unwrap_or(ExitCode::FAILURE));
    }
}
