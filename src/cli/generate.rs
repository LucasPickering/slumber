use crate::{
    cli::{request::BuildRequestCommand, Subcommand},
    template::TemplateError,
    GlobalArgs,
};
use async_trait::async_trait;
use clap::{Parser, ValueEnum};
use std::process::ExitCode;

/// Render a request and generate an equivalent for a third-party client
#[derive(Clone, Debug, Parser)]
#[clap(alias = "gen")]
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

#[async_trait]
impl Subcommand for GenerateCommand {
    async fn execute(self, global: GlobalArgs) -> anyhow::Result<ExitCode> {
        let (_, request) = self
            .build_request
            // User has to explicitly opt into executing triggered requests
            .build_request(global, self.execute_triggers)
            .await
            .map_err(|error| {
                // If the build failed because triggered requests are disabled,
                // replace it with a custom error message
                if TemplateError::has_trigger_disabled_error(&error) {
                    error.context(
                        "Triggered requests are disabled by default; \
                         pass `--execute-triggers` to enable",
                    )
                } else {
                    error
                }
            })?;
        println!("{}", request.to_curl()?);
        Ok(ExitCode::SUCCESS)
    }
}
