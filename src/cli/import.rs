use crate::{cli::Subcommand, collection::Collection, GlobalArgs};
use anyhow::Context;
use async_trait::async_trait;
use clap::{Parser, ValueEnum};
use std::{
    fs::File,
    io::{self, Write},
    path::PathBuf,
    process::ExitCode,
};

/// Generate a Slumber request collection from an external format
#[derive(Clone, Debug, Parser)]
pub struct ImportCommand {
    /// Input format
    format: Format,
    /// Collection to import
    input_file: PathBuf,
    /// Destination for the new slumber collection file [default: stdout]
    output_file: Option<PathBuf>,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum Format {
    Insomnia,
    /// A Jetbrains `.http` file in the REST format (without Jetbrains env files)
    Jetbrains,
    /// A VSCode `.rest` file in the REST format
    Vscode,
}

#[async_trait]
impl Subcommand for ImportCommand {
    async fn execute(self, _global: GlobalArgs) -> anyhow::Result<ExitCode> {
        // Load the input
        let collection = match self.format {
            Format::Insomnia => Collection::from_insomnia(&self.input_file)?,
            Format::Vscode => Collection::from_vscode(&self.input_file)?,
            Format::Jetbrains => Collection::from_jetbrains(&self.input_file)?,
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
                        {output_file:?}"
                    ))?,
            ),
            None => Box::new(io::stdout()),
        };
        serde_yaml::to_writer(&mut writer, &collection)?;

        Ok(ExitCode::SUCCESS)
    }
}
