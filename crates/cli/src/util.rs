use serde::Serialize;
use slumber_config::Config;
use slumber_core::util::confirm;
use std::{error::Error, io, iter, path::Path, process::ExitCode};

/// Print an error chain to stderr
pub fn print_error(error: &anyhow::Error) {
    eprintln!("{error}");
    error
        .chain()
        .skip(1)
        .for_each(|cause| eprintln!("  {cause}"));
}

/// Print rows in a table
pub fn print_table<const N: usize>(header: [&str; N], rows: &[[String; N]]) {
    // For each column, find the largest width of any cell
    let mut widths = [0; N];
    for column in 0..N {
        widths[column] = iter::once(header[column].len())
            .chain(rows.iter().map(|row| row[column].len()))
            .max()
            .unwrap_or_default()
            + 1; // Min width, for spacing
    }

    for (header, width) in header.into_iter().zip(widths.iter()) {
        print!("{header:<width$}");
    }
    println!();
    for row in rows {
        for (cell, width) in row.iter().zip(widths) {
            print!("{cell:<width$}");
        }
        println!();
    }
}

/// Serialize data to YAML and print it
///
/// ## Errors
///
/// Error if serialization fails or writing to stdout fails
pub fn print_yaml<T: Serialize>(value: &T) -> anyhow::Result<()> {
    // Panic is intentional, indicates a wonky bug
    serde_yaml::to_writer(io::stdout(), value).map_err(anyhow::Error::from)
}

/// Open a file in the user's configured editor. After the user closes the
/// editor, check if the file is valid using the given predicate. If it's
/// invalid, let the user know and offer to reopen it. This loop will repeat
/// indefinitely until the file is valid or the user chooses to exit.
pub fn edit_and_validate<T, E: 'static + Error + Send + Sync>(
    config: &Config,
    path: &Path,
    validate: impl Fn() -> Result<T, E>,
) -> anyhow::Result<ExitCode> {
    let editor = config.editor()?;
    loop {
        let status = editor.open(path).spawn()?.wait()?;

        // After editing, verify the file is valid. If not, offer to reopen
        if let Err(error) = validate() {
            // Convert to anyhow for display
            print_error(&error.into());
            if confirm(format!(
                "{path} is invalid, would you like to reopen it?",
                path = path.display(),
            )) {
                continue;
            }
        }

        // https://doc.rust-lang.org/stable/std/process/struct.ExitStatus.html#differences-from-exitcode
        let code = status.code().and_then(|code| u8::try_from(code).ok());
        return Ok(code.map(ExitCode::from).unwrap_or(ExitCode::FAILURE));
    }
}
