use crate::{GlobalArgs, Subcommand};
use anyhow::Context;
use clap::Parser;
use slumber_core::database::Database;
use std::process::ExitCode;
use tokio::process::Command;

/// Access the local Slumber database file.
///
/// This is an advanced command; most users never need to manually view or
/// modify the database file. By default this executes `sqlite3` and thus
/// requires `sqlite3` to be installed. You can customize which binary to invoke
/// with `--shell`. Read more about the Slumber database:
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
#[clap(verbatim_doc_comment)]
pub struct DbCommand {
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
}

impl Subcommand for DbCommand {
    async fn execute(self, _: GlobalArgs) -> anyhow::Result<ExitCode> {
        // Open a shell
        let path = Database::path();
        let exit_status = Command::new(self.exec)
            .arg(&path)
            .args(self.args)
            .spawn()
            .with_context(|| {
                format!("Error opening database file `{}`", path.display())
            })?
            .wait()
            .await?;

        // Forward exit code if we can, otherwise just match success/failure
        if let Some(code) =
            exit_status.code().and_then(|code| u8::try_from(code).ok())
        {
            Ok(code.into())
        } else if exit_status.success() {
            Ok(ExitCode::SUCCESS)
        } else {
            Ok(ExitCode::FAILURE)
        }
    }
}
