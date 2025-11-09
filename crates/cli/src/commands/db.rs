//! All operations related to direct DB modification live in here. Since these
//! are fairly niche and advanced, we group them all together to not pollute
//! the global command namespace with useless stuff.

mod collection;
mod request;

use crate::{
    GlobalArgs, Subcommand,
    commands::db::{
        collection::DbCollectionCommand, request::DbRequestCommand,
    },
};
use anyhow::Context;
use clap::Parser;
use slumber_core::database::Database;
use std::process::ExitCode;
use tokio::process::Command;

/// Access and modify the Slumber database (collection and request history)
///
/// Without a subcommand, this opens a shell into the database file. This is
/// advanced functionality; most users never need to manually view or modify the
/// database file. By default this executes `sqlite3` and thus requires
/// `sqlite3` to be installed. You can customize which binary to invoke with
/// `--shell`. Read more about the Slumber database:
///
/// https://slumber.lucaspickering.me/user_guide/database.html
///
/// This is simply an alias to make it easy to run your preferred SQLite shell
/// against the Slumber database. These two commands are equivalent:
///
///   slumber db -s <shell> -- <args...>
///
///   <shell> <path> <...args>
///
/// Where `<path>` is the path to the database file.
///
/// EXAMPLES:
///
/// Open a shell to the database:
///
///   slumber db
///
/// Run a single query and exit:
///
///   slumber db 'select 1'
#[derive(Clone, Debug, Parser)]
#[expect(rustdoc::invalid_html_tags)]
#[expect(rustdoc::bare_urls)]
#[clap(verbatim_doc_comment)]
pub struct DbCommand {
    #[command(subcommand)]
    subcommand: Option<DbSubcommand>,
    /// Program to execute
    #[clap(short = 'x', long, default_value = "sqlite3")]
    exec: String,
    /// Additional arguments to forward to the invoked program. Positional
    /// arguments can be passed like so:
    ///
    ///   slumber db 'select 1'
    ///
    /// However if you want to pass flags that begin with "-", you have to
    /// precede the forwarded arguments with "--" to separate them from
    /// arguments intended for `slumber`.
    ///
    ///   slumber db -- -cmd 'select 1'
    #[clap(num_args = 1.., verbatim_doc_comment)]
    args: Vec<String>,
    /// Print the path of the database file and exit; overrides all other
    /// arguments
    #[clap(long)]
    path: bool,
}

#[derive(Clone, Debug, clap::Subcommand)]
enum DbSubcommand {
    #[command(visible_alias = "coll")]
    Collection(DbCollectionCommand),
    #[command(visible_alias = "rq")]
    Request(DbRequestCommand),
}

impl Subcommand for DbCommand {
    async fn execute(self, global: GlobalArgs) -> anyhow::Result<ExitCode> {
        match self.subcommand {
            None => {
                let path = Database::path();

                if self.path {
                    println!("{}", path.display());
                    return Ok(ExitCode::SUCCESS);
                }

                // Open a shell
                let exit_status = Command::new(self.exec)
                    .arg(&path)
                    .args(self.args)
                    .spawn()
                    .with_context(|| {
                        format!(
                            "Error opening database file `{}`",
                            path.display()
                        )
                    })?
                    .wait()
                    .await?;

                // Forward exit code if we can, otherwise just do success/fail
                let exit_code =
                    exit_status.code().and_then(|code| u8::try_from(code).ok());
                if let Some(code) = exit_code {
                    Ok(code.into())
                } else if exit_status.success() {
                    Ok(ExitCode::SUCCESS)
                } else {
                    Ok(ExitCode::FAILURE)
                }
            }
            Some(DbSubcommand::Collection(command)) => {
                command.execute(global).await
            }
            Some(DbSubcommand::Request(command)) => {
                command.execute(global).await
            }
        }
    }
}
