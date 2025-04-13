use crate::{GlobalArgs, Subcommand};
use anyhow::Context;
use clap::{Parser, ValueEnum};
use std::{
    fs::File,
    io::{self, Write},
    path::PathBuf,
    process::ExitCode,
};

/// Generate a Slumber request collection from an external format
///
/// See docs for more info on formats:
/// https://slumber.lucaspickering.me/book/cli/import.html
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
#[allow(rustdoc::bare_urls)]
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
        // Load the input into a common import format. This is not the same as
        // the collection format actually used within Slumber, because that
        // format contains PS values that can't be turned back into source code.
        // Instead we use a declarative format similar to the YAML-based format
        // pre-Slumber v4.
        let collection: slumber_import::common::Collection = match self.format {
            Format::Insomnia => {
                slumber_import::from_insomnia(&self.input_file)?
            }
            Format::Openapi => {
                todo!()
                //  slumber_import::from_openapi(&self.input_file)?
            }
            Format::Rest => {
                todo!()
                // slumber_import::from_rest(&self.input_file)?
            }
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

        // Convert the declarative format into a PS AST, and write that to
        // source code
        todo!("Generate PS source code");

        Ok(ExitCode::SUCCESS)
    }
}
