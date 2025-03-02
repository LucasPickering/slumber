//! JS functions provided to users, to be used in collections

use crate::{
    collection::RecipeId,
    http::{Exchange, RequestSeed, ResponseRecord},
    js::{cereal, error::FunctionError},
    template::{
        Prompt, Renderer, Select, TemplateContext, TriggeredRequestError,
    },
};
use bytes::Bytes;
use chrono::Utc;
use petit_js::{Engine, Process, Value, serde::SerdeJs};
use reqwest::header::HeaderValue;
use serde::Deserialize;
use std::{path::PathBuf, process::Stdio, sync::Arc, time::Duration};
use tokio::{
    fs, io::AsyncWriteExt, process::Command, runtime::Handle, sync::oneshot,
};
use tracing::{debug, debug_span};

// TODO eliminate need for clones across lang barrier

/// Wrap an async function to make it sync, by spawning it on the runtime and
/// blocking on that task
/// TODO turn this into a func instead?
macro_rules! sync {
    ($f:expr) => {
        |process, args| {
            let future = $f(process, args);
            let rt = Handle::current();
            rt.block_on(future)
        }
    };
}

/// TODO
pub fn register_all(engine: &mut Engine) {
    engine.register_fn("select", sync!(select));
    engine.register_fn("command", sync!(command));
    engine.register_fn("env", env);
    engine.register_fn("file", sync!(file));
    engine.register_fn("profile", sync!(profile));
    engine.register_fn("prompt", sync!(prompt));
    engine.register_fn("response", sync!(response));
    engine.register_fn("responseHeader", sync!(response_header));
}

#[derive(Deserialize)]
struct SelectKwargs {
    message: String,
    options: Vec<String>,
}

/// Ask the user to select a value from a list
async fn select(
    process: &Process,
    SerdeJs(kwargs): SerdeJs<SelectKwargs>,
) -> Result<String, FunctionError> {
    let (tx, rx) = oneshot::channel();
    context(process)?.prompter.select(Select {
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
    _: &Process,
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
    Ok(trimmed)
}

/// Load the value of an environment variable
fn env(_: &Process, variable: String) -> Result<String, FunctionError> {
    Ok(std::env::var(variable).unwrap_or_default())
}

/// Load the contents of a file
async fn file(_: &Process, path: PathBuf) -> Result<Vec<u8>, FunctionError> {
    let output = fs::read(&path)
        .await
        .map_err(|error| FunctionError::File { path, error })?;
    Ok(output)
}

/// TODO
async fn profile(
    process: &Process,
    field: String,
) -> Result<Value, FunctionError> {
    let Some(template) = context(process)?
        .profile()
        .and_then(|profile| profile.data.get(&field))
    else {
        return Ok(Value::Undefined);
    };
    // Recursion!
    let renderer = Renderer::forked(process);
    renderer
        .render_value(template)
        .await
        .map_err(|error| FunctionError::FieldNested { field, error })
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
    process: &Process,
    SerdeJs(kwargs): SerdeJs<PromptKwargs>,
) -> Result<String, FunctionError> {
    let (tx, rx) = oneshot::channel();
    context(process)?.prompter.prompt(Prompt {
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
    process: &Process,
    (recipe_id, SerdeJs(kwargs)): (RecipeId, SerdeJs<ResponseKwargs>),
) -> Result<Bytes, FunctionError> {
    let response = context(process)?
        .get_latest_response(process, &recipe_id, kwargs.trigger)
        .await?;
    Ok(response.body.into_bytes())
}

#[derive(Deserialize)]
struct ResponseHeaderKwargs {
    #[serde(default)]
    trigger: RequestTrigger,
}

/// Load a header value from the most recent response for a recipe and the
/// current profile
async fn response_header(
    process: &Process,
    (recipe_id, header, SerdeJs(kwargs)): (
        RecipeId,
        String,
        SerdeJs<ResponseHeaderKwargs>,
    ),
) -> Result<Bytes, FunctionError> {
    let mut response = context(process)?
        .get_latest_response(process, &recipe_id, kwargs.trigger)
        .await?;
    let header: HeaderValue = response
        .headers
        .remove(&header)
        .ok_or_else(|| FunctionError::ResponseMissingHeader { header })?;
    // HeaderValue doesn't expose any way to move its bytes out so we have to
    // clone
    // https://github.com/hyperium/http/issues/661
    Ok(Bytes::copy_from_slice(header.as_bytes()))
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

/// Extract template context from the process's app data
fn context(process: &Process) -> Result<&TemplateContext, FunctionError> {
    process
        .app_data::<TemplateContext>()
        .map_err(|_| FunctionError::NoContext)
}

impl TemplateContext {
    /// Get the most recent response for a profile+recipe pair. This will
    /// trigger the request if it is expired, and await the response
    async fn get_latest_response(
        &self,
        process: &Process,
        recipe_id: &RecipeId,
        trigger: RequestTrigger,
    ) -> Result<ResponseRecord, FunctionError> {
        // Defer loading the most recent exchange until we know we'll need it
        let get_latest = || -> Result<Option<Exchange>, FunctionError> {
            self.database
                .get_latest_request(
                    self.selected_profile.as_ref().into(),
                    recipe_id,
                )
                .map_err(FunctionError::Database)
        };

        // Helper to execute the request, if triggered
        let send_request = || async {
            // There are 3 different ways we can generate the request config:
            // 1. Default (enable all query params/headers)
            // 2. Load from UI app_data for both TUI and CLI
            // 3. Load from UI app_data for TUI, enable all for CLI
            // These all have their own issues:
            // 1. Triggered request doesn't necessarily match behavior if user
            //  were to execute the request themself
            // 2. CLI behavior is silently controlled by UI app_data
            // 3. TUI and CLI behavior may not match
            // All 3 options are unintuitive in some way, but 1 is the easiest
            // to implement so I'm going with that for now.
            let build_options = Default::default();

            // Shitty try block
            async {
                // Fork the process so we can run a sub-render
                let renderer = Renderer::forked(process);
                let http_engine = self
                    .http_engine
                    .as_ref()
                    .ok_or(TriggeredRequestError::NotAllowed)?;
                let ticket = http_engine
                    .build(
                        RequestSeed::new(recipe_id.clone(), build_options),
                        &renderer,
                    )
                    .await
                    .map_err(|error| {
                        TriggeredRequestError::Build(error.into())
                    })?;
                ticket
                    .send(&self.database)
                    .await
                    .map_err(|error| TriggeredRequestError::Send(error.into()))
            }
            .await
            .map_err(|error| FunctionError::Trigger {
                recipe_id: recipe_id.clone(),
                error,
            })
        };

        let exchange = match trigger {
            RequestTrigger::Never => {
                get_latest()?.ok_or(FunctionError::ResponseMissing)?
            }
            RequestTrigger::NoHistory => {
                // If a exchange is present in history, use that. If not, fetch
                if let Some(exchange) = get_latest()? {
                    exchange
                } else {
                    send_request().await?
                }
            }
            RequestTrigger::Expire(duration) => match get_latest()? {
                Some(exchange)
                    if exchange.end_time + duration >= Utc::now() =>
                {
                    exchange
                }
                _ => send_request().await?,
            },
            RequestTrigger::Always => send_request().await?,
        };

        Ok(Arc::into_inner(exchange.response).expect("Arc was just created"))
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
