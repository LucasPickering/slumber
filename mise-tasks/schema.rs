#!/usr/bin/env -S cargo +nightly-2026-02-20 -Zscript
---
[package]
edition = "2024"

[dependencies]
# Script doesn't support workspaces yet so we have to redefine deps
clap = {version = "4", default-features = false, features = ["derive", "std"]}
schemars = {version = "=1.2.3", features = ["preserve_order"]}
slumber_config = {path = "../crates/config", features = ["schema", "tui"]}
slumber_core = {path = "../crates/core", features = ["schema"]}
serde = "=1.0.228"
serde_json = {version = "=1.0.149", features = ["preserve_order"]}
---
//! Script to generate JSON Schemas for Slumber entities. This is a separate
//! script because:
//! - It's only needed for developers, so it doesn't belong in the CLI
//! - Script minimizes the amount of code needed (compared to a crate)

use clap::{Parser, ValueEnum};
use slumber_config::Config;
use slumber_core::collection::Collection;
use std::{
    fmt::Display,
    fs, io,
    io::Write,
    path::Path,
    process::{Command, ExitCode, Stdio},
};

#[derive(Debug, Parser)]
#[clap(name = "slumber-schema")]
pub struct Args {
    /// Kind of schema(s) to generate. Omit to generate all
    #[clap(num_args = 0..)]
    schema: Vec<SchemaTarget>,
    /// Directory to output schema file(s) to. Pass `-` to print to stdout
    #[clap(long = "output", short = 'o', default_value = "schemas/")]
    output: String,
    /// Check if the generated schema has changed compared to the output files
    #[clap(long)]
    check: bool,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum SchemaTarget {
    Collection,
    Config,
}

impl Display for SchemaTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SchemaTarget::Collection => write!(f, "collection"),
            SchemaTarget::Config => write!(f, "config"),
        }
    }
}

fn main() -> ExitCode {
    let args = Args::parse();

    let targets = if args.schema.is_empty() {
        vec![SchemaTarget::Collection, SchemaTarget::Config]
    } else {
        args.schema
    };

    let mut has_check_error = false;
    for schema_target in targets {
        // Generate the JSON content
        let schema = match schema_target {
            SchemaTarget::Collection => schemars::schema_for!(Collection),
            SchemaTarget::Config => schemars::schema_for!(Config),
        };
        let json = serde_json::to_string_pretty(&schema).unwrap();

        // Either print the output or check it against the existing file
        if &args.output == "-" {
            if args.check {
                eprintln!("--check is not supported with stdout output");
                return ExitCode::FAILURE;
            } else {
                println!("{json}");
            }
        } else {
            let path =
                Path::new(&args.output).join(format!("{schema_target}.json"));
            if args.check {
                let old = fs::read_to_string(&path).unwrap();
                if old != json {
                    has_check_error = true;
                    eprintln!(
                        "{path} has changed; regenerate it with \
                        `mise run schema {schema_target}`",
                        path = path.display()
                    );

                    // Print a diff
                    if let Err(error) = print_diff(&path, &json) {
                        eprintln!("Error printing diff: {error}")
                    }
                }
            } else {
                println!("Writing to {}", path.display());
                fs::write(&path, &json).unwrap();
            }
        }
    }

    if has_check_error {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// Print the diff between a file (old) and in-memory content (new)
fn print_diff(file: &Path, new: &str) -> io::Result<()> {
    // Using git is much easier than calculating it ourselves. The new version
    // is piped in via stdin
    let mut child = Command::new("git")
        .args([
            "diff".as_ref(),
            "--no-index".as_ref(),
            "--".as_ref(),
            file.as_os_str(),
            "-".as_ref(),
        ])
        .stdin(Stdio::piped())
        .spawn()?;

    let stdin = child.stdin.as_mut().unwrap();
    stdin.write_all(new.as_bytes())?;
    child.wait()?;
    Ok(())
}
