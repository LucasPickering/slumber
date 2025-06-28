use crate::{GlobalArgs, Subcommand};
use anyhow::Context;
use clap::{Parser, ValueEnum};
use slumber_import::ImportInput;
use std::{
    fs::File,
    io::{self, Write},
    path::PathBuf,
    process::ExitCode,
    str::FromStr,
};
use tracing::info;

/// Generate a Slumber request collection from an external format
///
/// See docs for more info on formats:
/// https://slumber.lucaspickering.me/book/cli/import.html
#[derive(Clone, Debug, Parser)]
pub struct ImportCommand {
    /// Input format
    format: Format,
    /// File to import (path or URL)
    #[clap(value_parser = ImportInput::from_str)]
    input: ImportInput,
    /// Destination for the new slumber collection file [default: stdout]
    output_file: Option<PathBuf>,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum Format {
    /// Insomnia export format (JSON or YAML)
    Insomnia,
    /// OpenAPI v3.0 (JSON or YAML) v3.1 not supported but may work
    Openapi,
    /// VSCode `.rest` or JetBrains `.http` format [aliases: vscode, jetbrains]
    // Use visible_alias (and remove from doc comment) after
    // https://github.com/clap-rs/clap/pull/5480
    #[value(alias = "vscode", alias = "jetbrains")]
    Rest,
}

impl Subcommand for ImportCommand {
    async fn execute(self, _global: GlobalArgs) -> anyhow::Result<ExitCode> {
        info!(
            input = ?self.input, format = ?self.format, "Importing collection"
        );
        let collection = match self.format {
            Format::Insomnia => {
                slumber_import::from_insomnia(&self.input).await?
            }
            Format::Openapi => {
                slumber_import::from_openapi(&self.input).await?
            }
            Format::Rest => slumber_import::from_rest(&self.input).await?,
        };

        // Write the output
        let mut writer: Box<dyn Write> = match self.output_file {
            Some(output_file) => Box::new(
                File::options()
                    .create(true)
                    .truncate(true)
                    .write(true)
                    .open(&output_file)
                    .context(format!(
                        "Error opening collection output file \
                        `{}`",
                        output_file.display()
                    ))?,
            ),
            None => Box::new(io::stdout()),
        };
        serde_yaml::to_writer(&mut writer, &collection)
            .context(format!("Error loading collection from {}", self.input))?;

        Ok(ExitCode::SUCCESS)
    }
}
