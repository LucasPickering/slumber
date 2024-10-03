use crate::{Args, GlobalArgs, Subcommand, COMMAND_NAME};
use anyhow::anyhow;
use clap::{CommandFactory, Parser};
use clap_complete::Shell;
use std::{io, process::ExitCode};

/// Generate shell completions
#[derive(Clone, Debug, Parser)]
pub struct CompleteCommand {
    /// Shell type. Default to $SHELL
    #[clap(long)]
    shell: Option<Shell>,
}

impl Subcommand for CompleteCommand {
    async fn execute(self, _: GlobalArgs) -> anyhow::Result<ExitCode> {
        let shell = self
            .shell
            .or_else(Shell::from_env)
            .ok_or_else(|| anyhow!("No shell provided and none detected"))?;
        let mut command = Args::command();

        clap_complete::generate(
            shell,
            &mut command,
            COMMAND_NAME,
            &mut io::stdout(),
        );

        Ok(ExitCode::SUCCESS)
    }
}
