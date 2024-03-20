use crate::{
    cli::{request::BuildRequestCommand, Subcommand},
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
}

/// Third-party client to generate for
#[derive(Clone, Debug, ValueEnum)]
pub enum GenerateFormat {
    Curl,
}

#[async_trait]
impl Subcommand for GenerateCommand {
    async fn execute(self, global: GlobalArgs) -> anyhow::Result<ExitCode> {
        let (_, request) = self.build_request.build_request(global).await?;
        println!("{}", request.to_curl()?);
        Ok(ExitCode::SUCCESS)
    }
}
