use crate::{cli::Subcommand, collection::Collection, GlobalArgs};
use anyhow::Context;
use async_trait::async_trait;
use clap::Parser;
use std::{
    fs::File,
    io::{self, Write},
    path::PathBuf,
    process::ExitCode,
};

/// Generate a Slumber request collection from an external format
#[derive(Clone, Debug, Parser)]
pub struct ImportCommand {
    /// Collection to import
    input_file: PathBuf,
    /// Destination for the new slumber collection file. Omit to print to
    /// stdout.
    output_file: Option<PathBuf>,
}

#[async_trait]
impl Subcommand for ImportCommand {
    async fn execute(self, _global: GlobalArgs) -> anyhow::Result<ExitCode> {
        // Load the input
        let collection = Collection::from_insomnia(&self.input_file)?;

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
