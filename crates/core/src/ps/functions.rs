//! JS functions provided to users, to be used in collections

use crate::{
    collection::RecipeId,
    http::{RequestSeed, ResponseRecord},
    ps::{cereal, error::FunctionError},
    template::{
        OverrideKey, OverrideValue, Prompt, RenderContext, RenderState,
        Renderer, Select,
    },
    util::FutureCacheOutcome,
};
use bytes::Bytes;
use chrono::Utc;
use indexmap::indexmap;
use petitscript::{Engine, Exports, FromPs, Process, Value, error::ValueError};
use serde::{Deserialize, de::IntoDeserializer};
use std::{path::PathBuf, process::Stdio, sync::Arc, time::Duration};
use tokio::{
    fs, io::AsyncWriteExt, process::Command, runtime::Handle, sync::oneshot,
};
use tracing::{debug, debug_span};

// TODO eliminate need for clones across lang barrier

/// Wrap an async function to make it sync by spawning it on the runtime and
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

/// Create the `slumber` module and register it in with the engine
pub fn register_module(engine: &mut Engine) {
    let functions = indexmap! {
        "command" => engine.create_fn(sync!(command)),
        "env" => engine.create_fn(env),
        "file" => engine.create_fn(sync!(file)),
        "profile" => engine.create_fn(sync!(profile)),
        "prompt" => engine.create_fn(sync!(prompt)),
        "response" => engine.create_fn(sync!(response)),
        "responseHeader" => engine.create_fn(sync!(response_header)),
        "select" => engine.create_fn(sync!(select)),
    };
    // This only fails if the name is invalid
    engine
        .register_module("slumber", Exports::named_and_default(functions))
        .unwrap();
}

#[derive(Default, Deserialize)]
struct CommandKwargs {
    /// Optional data to pipe to the command via stdin
    #[serde(default)]
    stdin: Option<String>,
    /// Decoding mode - text or binary?
    #[serde(default)]
    decode: Decoding,
    /// Trim whitespace from beginning/end of output
    #[serde(default)]
    trim: TrimMode,
}

/// Run a command in a subprocess
async fn command(
    _: &Process,
    (
        command,
        Kwargs(CommandKwargs {
            stdin,
            decode: encoding,
            trim,
        }),
    ): (Vec<String>, Kwargs<CommandKwargs>),
) -> Result<Value, FunctionError> {
    let [program, args @ ..] = command.as_slice() else {
        return Err(FunctionError::Argument(
            "command must have at least one element".into(),
        ));
    };
    let _ = debug_span!("Executing command", ?program, ?args).entered();

    let output = async {
        // Spawn the command process
        let mut process = Command::new(program)
            .args(args)
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
        program: program.into(),
        args: args.into(),
        error,
    })?;

    debug!(
        stdout = %String::from_utf8_lossy(&output.stdout),
        stderr = %String::from_utf8_lossy(&output.stderr),
        "Command success"
    );

    let trimmed = trim.apply(output.stdout);
    encoding.decode(trimmed.into())
}

/// Load the value of an environment variable
fn env(_: &Process, variable: String) -> String {
    std::env::var(variable).unwrap_or_default()
}

/// Load the contents of a file
async fn file(_: &Process, path: PathBuf) -> Result<Vec<u8>, FunctionError> {
    let output = fs::read(&path)
        .await
        .map_err(|error| FunctionError::File { path, error })?;
    Ok(output)
}

/// Access a field in the current profile. If the field is a function, call the
/// function to render it recursively, then return its return value. Multiple
/// calls to this function within the same recipe render will be cached, meaning
/// each profile field will be rendered no more than once, and the result will
/// be shared between all consumers of that field.
async fn profile(
    process: &Process,
    field: String,
) -> Result<Value, FunctionError> {
    let context = context(process)?;

    // Check if this field has been manually overridden
    match context
        .overrides
        .get(&OverrideKey::Profile(field.as_str().into()))
    {
        // Manually overridden - return the user's value
        Some(OverrideValue::Override(value)) => {
            return Ok(value.clone().into());
        }
        // Pretend the value isn't here
        Some(OverrideValue::Omit) => return Ok(Value::Undefined),
        None => {} // Move along nothing to see here
    }

    let profile = context.profile();
    let state = state(process)?;
    // Check the cache to see if this value is already being computed somewhere
    // else. If it is, we'll block on that and re-use the result. If not, we
    // get a guard back, meaning we're responsible for the computation. At
    // the end, we'll write back to the guard so everyone else can copy our
    // homework.
    let guard = match state.profile_cache.get_or_init(field.clone()).await {
        FutureCacheOutcome::Hit(result) => {
            return result
                .map_err(|error| FunctionError::FieldNested { field, error });
        }
        FutureCacheOutcome::Miss(guard) => guard,
        FutureCacheOutcome::NoResponse => {
            // This is possible if the first responder panicked while holding
            // write lock. We could try again here but the rest of the render
            // is going to fail anyway so there isn't much point. Since this
            // is run in a subtask, the panic won't kill the whole program.
            panic!("Cached future did not set a value. This is a bug!")
        }
    };
    let result = if let Some(template) =
        profile.and_then(|profile| profile.data.get(&field))
    {
        // Recursion!
        let renderer = Renderer::forked(process);
        renderer.render::<Value>(template).await.map_err(Arc::new)
    } else {
        Ok(Value::Undefined)
    };
    guard.set(result.clone());
    result.map_err(|error| FunctionError::FieldNested { field, error })
}

#[derive(Default, Deserialize)]
struct PromptKwargs {
    default: Option<String>,
    #[serde(default)]
    sensitive: bool,
}

/// Prompt the user to enter a text value
async fn prompt(
    process: &Process,
    (message, Kwargs(PromptKwargs { default, sensitive })): (
        String,
        Kwargs<PromptKwargs>,
    ),
) -> Result<String, FunctionError> {
    let (tx, rx) = oneshot::channel();
    context(process)?.prompter.prompt(Prompt {
        message,
        default,
        sensitive,
        channel: tx.into(),
    });
    let output = rx.await.map_err(|_| FunctionError::PromptNoReply)?;
    Ok(output)
}

#[derive(Default, Deserialize)]
struct ResponseKwargs {
    /// Decoding mode - text or binary?
    #[serde(default)]
    decode: Decoding,
    /// If/when should we trigger an upstream request?
    #[serde(default)]
    trigger: RequestTrigger,
}

/// Load the most recent response body for a recipe and the current profile
async fn response(
    process: &Process,
    (recipe_id, Kwargs(kwargs)): (RecipeId, Kwargs<ResponseKwargs>),
) -> Result<Value, FunctionError> {
    let ResponseKwargs { decode, trigger } = kwargs;
    let response = context(process)?
        .get_latest_response(process, &recipe_id, trigger)
        .await?;
    let body = match Arc::try_unwrap(response) {
        Ok(response) => response.body,
        Err(response) => response.body.clone(),
    };
    decode.decode(body.into_bytes())
}

#[derive(Default, Deserialize)]
struct ResponseHeaderKwargs {
    /// Decoding mode - text or binary?
    #[serde(default)]
    decode: Decoding,
    /// If/when should we trigger an upstream request?
    #[serde(default)]
    trigger: RequestTrigger,
}

/// Load a header value from the most recent response for a recipe and the
/// current profile
async fn response_header(
    process: &Process,
    (recipe_id, header, Kwargs(kwargs)): (
        RecipeId,
        String,
        Kwargs<ResponseHeaderKwargs>,
    ),
) -> Result<Value, FunctionError> {
    let ResponseHeaderKwargs { decode, trigger } = kwargs;
    let response = context(process)?
        .get_latest_response(process, &recipe_id, trigger)
        .await?;
    // Only clone the header value if necessary
    let header_value = match Arc::try_unwrap(response) {
        Ok(mut response) => response.headers.remove(&header),
        Err(response) => response.headers.get(&header).cloned(),
    }
    .ok_or_else(|| FunctionError::ResponseMissingHeader { header })?;
    // HeaderValue doesn't expose any way to move its bytes out so we must clone
    // https://github.com/hyperium/http/issues/661
    decode.decode(Bytes::copy_from_slice(header_value.as_bytes()))
}

/// Ask the user to select a value from a list
async fn select(
    process: &Process,
    (message, options): (String, Vec<String>),
) -> Result<String, FunctionError> {
    let (tx, rx) = oneshot::channel();
    context(process)?.prompter.select(Select {
        message,
        options,
        channel: tx.into(),
    });
    let output = rx.await.map_err(|_| FunctionError::PromptNoReply)?;
    Ok(output)
}

/// Wrapper for a keyword argument struct, which will be deserialized from a
/// a PS object. Kwargs should only be used for additional options to a function
/// that are not required. As such `T` must implement `Default` to define a
/// fallback for all fields when the argument isn't passed.
struct Kwargs<T>(T);

impl<'de, T: Default + Deserialize<'de>> FromPs for Kwargs<T> {
    fn from_ps(value: Value) -> Result<Self, ValueError> {
        match value {
            // If the arg wasn't passed, fall back to the default
            Value::Undefined => Ok(Self(T::default())),
            // Deserialize the value as the kwarg struct
            _ => {
                let deserializer = value.into_deserializer();
                serde_path_to_error::deserialize(deserializer)
                    .map(Self)
                    .map_err(ValueError::other)
            }
        }
    }
}

/// Define when a recipe with a chained request should auto-execute the
/// dependency request.
#[derive(Copy, Clone, Debug, Default, Deserialize)]
#[cfg_attr(any(test, feature = "test"), derive(PartialEq))]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
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
    Expire {
        #[serde(deserialize_with = "cereal::deserialize_duration")]
        duration: Duration,
    },
    /// Trigger the request every time the dependent request is rendered
    Always,
}

/// Trim whitespace from rendered output
#[derive(Copy, Clone, Debug, Default, Deserialize)]
#[cfg_attr(any(test, feature = "test"), derive(PartialEq))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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

/// TODO better name
#[derive(Copy, Clone, Debug, Default, Deserialize)]
#[cfg_attr(any(test, feature = "test"), derive(PartialEq))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
enum Decoding {
    /// Load data as a UTF-8 string
    #[default]
    Text,
    /// Load data as raw bytes
    Binary,
}

impl Decoding {
    /// Decode bytes according to this encoding scheme, and return as a Petit
    /// value
    fn decode(self, bytes: Bytes) -> Result<Value, FunctionError> {
        match self {
            Self::Text => String::from_utf8(bytes.into())
                .map(Value::from)
                .map_err(FunctionError::InvalidUtf8),
            Self::Binary => Ok(bytes.into()),
        }
    }
}

/// Extract template context from the process's app data
fn context(process: &Process) -> Result<&RenderContext, FunctionError> {
    process
        .app_data::<RenderContext>()
        .map_err(|_| FunctionError::NoContext)
}

/// Extract render state from the process's app data
fn state(process: &Process) -> Result<&RenderState, FunctionError> {
    process
        .app_data::<RenderState>()
        .map_err(|_| FunctionError::NoContext)
}

impl RenderContext {
    /// Get the most recent response for a profile+recipe pair. This will
    /// trigger the request if it is expired, and await the response
    async fn get_latest_response(
        &self,
        process: &Process,
        recipe_id: &RecipeId,
        trigger: RequestTrigger,
    ) -> Result<Arc<ResponseRecord>, FunctionError> {
        // Defer loading the most recent exchange until we know we'll need it
        let get_latest = || async {
            self.http_provider
                .get_latest_request(self.selected_profile.as_ref(), recipe_id)
                .await
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
            // TODO move this comment somewhere more appropriate ^

            // Fork the process so we can run a sub-render
            let renderer = Renderer::forked(process);
            self.http_provider
                .send_request(RequestSeed::new(recipe_id.clone()), &renderer)
                .await
                .map_err(|error| FunctionError::Trigger {
                    recipe_id: recipe_id.clone(),
                    error,
                })
        };

        let exchange = match trigger {
            RequestTrigger::Never => {
                get_latest().await?.ok_or(FunctionError::ResponseMissing)?
            }
            RequestTrigger::NoHistory => {
                // If a exchange is present in history, use that. If not, fetch
                if let Some(exchange) = get_latest().await? {
                    exchange
                } else {
                    send_request().await?
                }
            }
            RequestTrigger::Expire { duration } => match get_latest().await? {
                Some(exchange)
                    if exchange.end_time + duration >= Utc::now() =>
                {
                    exchange
                }
                _ => send_request().await?,
            },
            RequestTrigger::Always => send_request().await?,
        };

        Ok(exchange.response)
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
