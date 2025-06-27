use crate::{GlobalArgs, Subcommand};
use clap::{Parser, ValueEnum};
use slumber_config::Config;
use slumber_core::collection::Collection;
use std::process::ExitCode;

/// Generate JSON Schema for Slumber collection and config types
#[derive(Clone, Debug, Parser)]
pub struct SchemaCommand {
    kind: SchemaKind,
}

/// Schema to generate
#[derive(Clone, Debug, ValueEnum)]
enum SchemaKind {
    /// Collection file schema, e.g. for `slumber.yml`
    Collection,
    /// Global config file schema
    Config,
}

impl Subcommand for SchemaCommand {
    // TODO fix doc comments in schema
    async fn execute(self, _global: GlobalArgs) -> anyhow::Result<ExitCode> {
        let schema = match self.kind {
            SchemaKind::Collection => schemars::schema_for!(Collection),
            SchemaKind::Config => schemars::schema_for!(Config),
        };
        let json = serde_json::to_string_pretty(&schema)?; // Should be infallible
        println!("{json}");
        Ok(ExitCode::SUCCESS)
    }
}
