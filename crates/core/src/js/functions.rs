//! JS functions provided to users, to be used in collections

use crate::{
    collection::RecipeId,
    js::{cereal, error::FunctionError},
    template::{Prompt, Select},
};
use bytes::Bytes;
use petit_js::{Engine, SerdeJs};
use reqwest::header::HeaderValue;
use serde::Deserialize;
use std::{path::PathBuf, process::Stdio, time::Duration};
use tokio::{fs, io::AsyncWriteExt, process::Command, sync::oneshot};
use tracing::{debug, debug_span};

// TODO eliminate need for clones across lang barrier

/// TODO
pub fn register_all(engine: &mut Engine) {
    engine.register_async_fn("select", select);
    engine.register_async_fn("command", command);
    engine.register_async_fn("env", env);
    engine.register_async_fn("file", file);
    engine.register_async_fn("prompt", prompt);
    engine.register_async_fn("response", response);
    engine.register_async_fn("responseHeader", response_header);
}

#[derive(Deserialize)]
struct SelectKwargs {
    message: String,
    options: Vec<String>,
}

/// Ask the user to select a value from a list
async fn select(
    SerdeJs(kwargs): SerdeJs<SelectKwargs>,
) -> Result<String, FunctionError> {
    let (tx, rx) = oneshot::channel();
    renderer.context().prompter.select(Select {
        message: kwargs.message,
        options: kwargs.options,
        channel: tx.into(),
    });
    let output = rx.await.map_err(|_| FunctionError::PromptNoReply)?;
    Ok(output)
}

#[derive(Deserialize)]
struct CommandKwargs {
    /// Name/path to the command
    command: String,
    /// Arguments to pass to the command
    #[serde(default)]
    args: Vec<String>,
    /// Optional data to pipe to the command via stdin
    stdin: Option<String>,
    /// Trim whitespace from beginning/end of output
    trim: TrimMode,
}

/// Run a command in a subprocess
async fn command(
    SerdeJs(kwargs): SerdeJs<CommandKwargs>,
) -> Result<Vec<u8>, FunctionError> {
    let CommandKwargs {
        command: program,
        args,
        stdin,
        trim,
    } = kwargs;
    let _ = debug_span!("Executing command", ?program, ?args).entered();

    let output = async {
        // Spawn the command process
        let mut process = Command::new(&program)
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        // Write the stdin to the process
        if let Some(stdin) = stdin {
            process
                .stdin
                .as_mut()
                .expect("Process missing stdin")
                .write_all(stdin.as_bytes())
                .await?;
        }

        // Wait for the process to finish
        process.wait_with_output().await
    }
    .await
    .map_err(|error| FunctionError::Command {
        program,
        args,
        error,
    })?;

    debug!(
        stdout = %String::from_utf8_lossy(&output.stdout),
        stderr = %String::from_utf8_lossy(&output.stderr),
        "Command success"
    );

    let trimmed = trim.apply(output.stdout);
    Ok(trimmed.into())
}

/// Load the value of an environment variable
async fn env(variable: String) -> Result<String, FunctionError> {
    Ok(std::env::var(variable).unwrap_or_default())
}

/// Load the contents of a file
async fn file(path: PathBuf) -> Result<Vec<u8>, FunctionError> {
    let output = fs::read(&path)
        .await
        .map_err(|error| FunctionError::File { path, error })?;
    Ok(output.into())
}

#[derive(Deserialize)]
struct PromptKwargs {
    message: String,
    default: Option<String>,
    #[serde(default)]
    sensitive: bool,
}

/// Prompt the user to enter a text value
async fn prompt(
    SerdeJs(kwargs): SerdeJs<PromptKwargs>,
) -> Result<String, FunctionError> {
    let (tx, rx) = oneshot::channel();
    renderer.context().prompter.prompt(Prompt {
        message: kwargs.message,
        default: kwargs.default,
        sensitive: kwargs.sensitive,
        channel: tx.into(),
    });
    let output = rx.await.map_err(|_| FunctionError::PromptNoReply)?;
    Ok(output)
}

#[derive(Deserialize)]
struct ResponseKwargs {
    #[serde(default)]
    trigger: RequestTrigger,
}

/// Load the most recent response body for a recipe and the current profile
async fn response(
    recipe_id: RecipeId,
    SerdeJs(kwargs): SerdeJs<ResponseKwargs>,
) -> Result<Bytes, FunctionError> {
    let response = renderer
        .get_latest_response(&recipe_id, kwargs.trigger)
        .await?;
    Ok(response.body.into_bytes().into())
}

#[derive(Deserialize)]
struct ResponseHeaderKwargs {
    #[serde(default)]
    trigger: RequestTrigger,
}

/// Load a header value from the most recent response for a recipe and the
/// current profile
async fn response_header(
    recipe_id: RecipeId,
    header: String,
    SerdeJs(kwargs): SerdeJs<ResponseHeaderKwargs>,
) -> Result<Bytes, FunctionError> {
    let mut response = renderer
        .get_latest_response(&recipe_id, kwargs.trigger)
        .await?;
    let header: HeaderValue = response
        .headers
        .remove(&header)
        .ok_or_else(|| FunctionError::ResponseMissingHeader { header })?;
    // HeaderValue doesn't expose any way to move its bytes out so we have to
    // clone
    Ok(header.as_bytes().into())
}

/// Define when a recipe with a chained request should auto-execute the
/// dependency request.
#[derive(Copy, Clone, Debug, Default, Deserialize)]
#[cfg_attr(any(test, feature = "test"), derive(PartialEq))]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum RequestTrigger {
    /// Never trigger the request. This is the default because upstream
    /// requests could be mutating, so we want the user to explicitly opt into
    /// automatic execution.
    #[default]
    Never,
    /// Trigger the request if there is none in history
    NoHistory,
    /// Trigger the request if the last response is older than some
    /// duration (or there is none in history)
    Expire(#[serde(with = "cereal::serde_duration")] Duration),
    /// Trigger the request every time the dependent request is rendered
    Always,
}

/// Trim whitespace from rendered output
#[derive(Copy, Clone, Debug, Default, Deserialize)]
#[cfg_attr(any(test, feature = "test"), derive(PartialEq))]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum TrimMode {
    /// Do not trim the output
    #[default]
    None,
    /// Trim the start of the output
    Start,
    /// Trim the end of the output
    End,
    /// Trim the start and end of the output
    Both,
}

impl TrimMode {
    /// Apply whitespace trimming to string values. If the value is not a valid
    /// string, no trimming is applied
    fn apply(self, value: Vec<u8>) -> Vec<u8> {
        // Theoretically we could strip whitespace-looking characters from
        // binary data, but if the whole thing isn't a valid string it doesn't
        // really make any sense to.
        let Ok(s) = std::str::from_utf8(&value) else {
            return value;
        };
        match self {
            Self::None => value,
            Self::Start => s.trim_start().into(),
            Self::End => s.trim_end().into(),
            Self::Both => s.trim().into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_choose() {
        todo!()
    }

    #[test]
    fn test_command() {
        todo!()
    }

    #[test]
    fn test_env() {
        todo!()
    }

    #[test]
    fn test_file() {
        todo!()
    }

    #[test]
    fn test_profile() {
        todo!()
    }

    #[test]
    fn test_prompt() {
        todo!()
    }

    #[test]
    fn test_response() {
        todo!()
    }

    #[test]
    fn test_response_header() {
        todo!()
    }
}
