use crate::{GlobalArgs, Subcommand, commands::request::BuildRequestCommand};
use clap::{Parser, ValueEnum};
use std::process::ExitCode;

/// Render a request and generate an equivalent for a third-party client
#[derive(Clone, Debug, Parser)]
#[clap(visible_alias = "gen")]
pub struct GenerateCommand {
    format: GenerateFormat,
    #[clap(flatten)]
    build_request: BuildRequestCommand,
    /// Execute triggered sub-requests. By default, if a request dependency is
    /// triggered (e.g. if it is expired), an error will be thrown instead
    #[clap(long)]
    execute_triggers: bool,
}

/// Third-party client to generate for
#[derive(Clone, Debug, ValueEnum)]
pub enum GenerateFormat {
    Curl,
}

impl Subcommand for GenerateCommand {
    async fn execute(self, global: GlobalArgs) -> anyhow::Result<ExitCode> {
        match self.format {
            GenerateFormat::Curl => {
                let (_, http_engine, seed, template_context) = self
                    .build_request
                    .build_seed(global, self.execute_triggers)?;
                let command = http_engine
                    .build_curl(seed, &template_context)
                    .await
                    .map_err(|error| {
                        // If the build failed because triggered requests are
                        // disabled, replace it with a custom error message
                        if error.has_trigger_disabled_error() {
                            anyhow::Error::from(error.error).context(
                                "Triggered requests are disabled by default; \
                                 pass `--execute-triggers` to enable",
                            )
                        } else {
                            error.error.into()
                        }
                    })?;
                println!("{command}");
            }
        }
        Ok(ExitCode::SUCCESS)
    }
}
