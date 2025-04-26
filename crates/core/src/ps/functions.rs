//! PetitScript functions that make up the slumber native module

use crate::{
    collection::RecipeId,
    http::{RequestSeed, ResponseRecord},
    ps::error::FunctionError,
    render::{
        OverrideKey, OverrideValue, Prompt, RenderContext, RenderState,
        Renderer, Select,
    },
    util::FutureCacheOutcome,
};
use bytes::Bytes;
use chrono::Utc;
use indexmap::indexmap;
use petitscript::{
    Engine, Exports, Process, Value,
    error::ValueError,
    value::{FromPetit, FromPetitArgs, IntoPetitResult},
};
use serde::{
    Deserialize,
    de::{self, IntoDeserializer, Visitor},
};
use serde_json_path::JsonPath;
use slumber_util::Duration;
use std::{path::PathBuf, process::Stdio, sync::Arc};
use tokio::{
    fs, io::AsyncWriteExt, process::Command, runtime::Handle, sync::oneshot,
};
use tracing::{debug, debug_span};

/// Create the `slumber` module and register it in the engine
pub fn register_module(engine: &mut Engine) {
    let functions = indexmap! {
        "command" => engine.create_fn(sync(command)),
        "env" => engine.create_fn(env),
        "file" => engine.create_fn(sync(file)),
        "jsonPath" => engine.create_fn(json_path),
        "profile" => engine.create_fn(sync(profile)),
        "prompt" => engine.create_fn(sync(prompt)),
        "response" => engine.create_fn(sync(response)),
        "responseHeader" => engine.create_fn(sync(response_header)),
        "select" => engine.create_fn(sync(select)),
        "sensitive" => engine.create_fn(sensitive),
    };
    engine
        .register_module("slumber", Exports::named(functions))
        // This only fails if the name is invalid
        .unwrap();
}

/// Wrap an async function to make it sync by spawning it on the runtime and
/// blocking on that task
fn sync<Args: FromPetitArgs, Out: IntoPetitResult>(
    f: impl 'static + AsyncFn(&Process, Args) -> Out,
) -> impl 'static + Fn(&Process, Args) -> Out {
    move |process, args| {
        let future = f(process, args);
        let rt = Handle::current();
        rt.block_on(future)
    }
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommandKwargs {
    /// Optional data to pipe to the command via stdin
    #[serde(default)]
    stdin: Option<String>,
    /// Decoding mode - text or binary?
    #[serde(default)]
    decode: Decoding,
}

/// Run a command in a subprocess
async fn command(
    _: &Process,
    (command, Kwargs(CommandKwargs { stdin, decode })): (
        Vec<String>,
        Kwargs<CommandKwargs>,
    ),
) -> Result<Value, FunctionError> {
    let [program, args @ ..] = command.as_slice() else {
        return Err(FunctionError::CommandEmpty);
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
        program: program.clone(),
        args: args.into(),
        error,
    })?;

    debug!(
        stdout = %String::from_utf8_lossy(&output.stdout),
        stderr = %String::from_utf8_lossy(&output.stderr),
        "Command success"
    );

    decode.decode(output.stdout.into())
}

/// Load the value of an environment variable
fn env(_: &Process, variable: String) -> String {
    std::env::var(variable).unwrap_or_default()
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileKwargs {
    /// Decoding mode - text or binary?
    #[serde(default)]
    decode: Decoding,
}

/// Load the contents of a file
async fn file(
    _: &Process,
    (path, Kwargs(FileKwargs { decode })): (PathBuf, Kwargs<FileKwargs>),
) -> Result<Value, FunctionError> {
    let output = fs::read(&path)
        .await
        .map_err(|error| FunctionError::File { path, error })?;

    decode.decode(output.into())
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct JsonPathKwargs {
    /// Modify how the query handles 0, 1, or 2+ results
    #[serde(default)]
    mode: JsonPathMode,
}

/// TODO
fn json_path(
    _: &Process,
    (Todo(query), Todo(value), Kwargs(JsonPathKwargs { mode })): (
        Todo<JsonPath>,
        Todo<serde_json::Value>,
        Kwargs<JsonPathKwargs>,
    ),
) -> Result<Value, FunctionError> {
    let node_list = query.query(&value);

    // Use the mode to determine how to handle 0, 1, or 2+ results
    let json: serde_json::Value = match mode {
        JsonPathMode::Auto => match node_list.len() {
            0 => return Err(FunctionError::JsonPathNoResults { query }),
            1 => node_list.first().unwrap().clone(),
            2.. => node_list.into_iter().cloned().collect(),
        },
        JsonPathMode::Single => node_list
            .exactly_one()
            .map_err(|_| FunctionError::JsonPathExactlyOne {
                query,
                actual_count: node_list.len(),
            })?
            .clone(),
        JsonPathMode::Array => node_list.into_iter().cloned().collect(),
    };

    // Convert from JSON back to PS. PS is a superset of JSON so this conversion
    // is infallible
    Ok(serde_json::from_value(json).unwrap())
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
            return result.map_err(|error| FunctionError::ProfileNested {
                field,
                error,
            });
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
    let result = if let Some(value) =
        profile.and_then(|profile| profile.data.get(&field))
    {
        // Recursion!
        let renderer = Renderer::forked(process);
        renderer.render::<Value>(value).await.map_err(Arc::new)
    } else {
        Ok(Value::Undefined)
    };
    guard.set(result.clone());
    result.map_err(|error| FunctionError::ProfileNested { field, error })
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PromptKwargs {
    message: Option<String>,
    default: Option<String>,
    /// Mask the prompt value while typing
    #[serde(default)]
    sensitive: bool,
}

/// Prompt the user to enter a text value
async fn prompt(
    process: &Process,
    Kwargs(PromptKwargs {
        message,
        default,
        sensitive,
    }): Kwargs<PromptKwargs>,
) -> Result<String, FunctionError> {
    let (tx, rx) = oneshot::channel();
    let context = context(process)?;
    context.prompter.prompt(Prompt {
        message: message.unwrap_or_default(),
        default,
        sensitive,
        channel: tx.into(),
    });
    let output = rx.await.map_err(|_| FunctionError::PromptNoReply)?;

    // If the input was sensitive, we should mask it for previews as well.
    // This is a little wonky because the preview prompter just spits out a
    // static string anyway, but it's "technically" right and plays well in
    // tests. Also it reminds users that a prompt is sensitive in the TUI :)
    if sensitive {
        Ok(mask_sensitive(context, output))
    } else {
        Ok(output)
    }
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
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
    (recipe_id, Kwargs(ResponseKwargs { decode, trigger })): (
        RecipeId,
        Kwargs<ResponseKwargs>,
    ),
) -> Result<Value, FunctionError> {
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
#[serde(deny_unknown_fields)]
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
    (recipe_id, header, Kwargs(ResponseHeaderKwargs { decode, trigger })): (
        RecipeId,
        String,
        Kwargs<ResponseHeaderKwargs>,
    ),
) -> Result<Value, FunctionError> {
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

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct SelectKwargs {
    message: Option<String>,
}

/// Ask the user to select a value from a list
async fn select(
    process: &Process,
    (options, Kwargs(SelectKwargs { message })): (
        Vec<String>,
        Kwargs<SelectKwargs>,
    ),
) -> Result<String, FunctionError> {
    let (tx, rx) = oneshot::channel();
    context(process)?.prompter.select(Select {
        message: message.unwrap_or_default(),
        options,
        channel: tx.into(),
    });
    let output = rx.await.map_err(|_| FunctionError::PromptNoReply)?;
    Ok(output)
}

/// Mark a string as sensitive. Sensitive strings will be masked in previews.
fn sensitive(
    process: &Process,
    value: String,
) -> Result<String, FunctionError> {
    let context = context(process)?;
    Ok(mask_sensitive(context, value))
}

/// Hide a sensitive value if the context has show_sensitive disabled
fn mask_sensitive(context: &RenderContext, value: String) -> String {
    if context.show_sensitive {
        value
    } else {
        "•".repeat(value.chars().count())
    }
}

/// TODO
struct Todo<T>(T);

impl<'de, T: Deserialize<'de>> FromPetit for Todo<T> {
    fn from_petit(value: Value) -> Result<Self, ValueError> {
        let deserializer = value.into_deserializer();
        serde_path_to_error::deserialize(deserializer)
            .map(Self)
            .map_err(ValueError::other)
    }
}

/// Wrapper for a keyword argument struct, which will be deserialized from a
/// a PS object. Kwargs should only be used for additional options to a function
/// that are not required. As such `T` must implement `Default` to define a
/// fallback for all fields when the argument isn't passed.
struct Kwargs<T>(T);

impl<'de, T: Default + Deserialize<'de>> FromPetit for Kwargs<T> {
    fn from_petit(value: Value) -> Result<Self, ValueError> {
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
#[derive(Copy, Clone, Debug, Default)]
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
    Expire { duration: Duration },
    /// Trigger the request every time the dependent request is rendered
    Always,
}

/// Deserialize a request trigger from a single string. Unit variants are
/// assigned a static string, and anything else is treated as an expire
/// duration.
impl<'de> Deserialize<'de> for RequestTrigger {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct RequestTriggerVisitor;

        impl Visitor<'_> for RequestTriggerVisitor {
            type Value = RequestTrigger;

            fn expecting(
                &self,
                formatter: &mut std::fmt::Formatter,
            ) -> std::fmt::Result {
                formatter.write_str("TODO")
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                match v {
                    "never" => Ok(RequestTrigger::Never),
                    "noHistory" => Ok(RequestTrigger::NoHistory),
                    "always" => Ok(RequestTrigger::Always),
                    // Anything else is parsed as a duration
                    _ => {
                        let duration =
                            v.parse::<Duration>().map_err(de::Error::custom)?;
                        Ok(RequestTrigger::Expire { duration })
                    }
                }
            }
        }

        deserializer.deserialize_any(RequestTriggerVisitor)
    }
}

/// TODO better name
#[derive(Default, Deserialize)]
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

/// Control how a JSONPath selector returns 0 vs 1 vs 2+ results
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
enum JsonPathMode {
    /// 0 - Error
    /// 1 - Single result, without wrapping quotes
    /// 2 - JSON array
    #[default]
    Auto,
    /// 0 - Error
    /// 1 - Single result, without wrapping quotes
    /// 2 - Error
    Single,
    /// 0 - JSON array
    /// 1 - JSON array
    /// 2 - JSON array
    Array,
}

/// Extract render context from the process's app data
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
                    if exchange.end_time + duration.inner() >= Utc::now() =>
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
    use crate::{
        collection::{Collection, Profile},
        database::CollectionDatabase,
        http::{Exchange, RequestRecord, ResponseBody},
        render::Overrides,
        test_util::{
            TestHttpProvider, TestPrompter, TestSelectPrompter, by_id,
            header_map,
        },
    };
    use petitscript::{Engine, Value, value::Function};
    use rstest::rstest;
    use slumber_util::{Factory, TempDir, assert_err, temp_dir};
    use std::{iter, sync::LazyLock};
    use tokio::task;

    // These test functions via PS rather than calling them directly so we can
    // test them from a user perspective. This covers extra logic like the
    // function name, arg deserialization, output conversion, etc.

    #[rstest]
    #[case::base([["echo", "hi"].into()], "hi\n")]
    #[case::stdin(
        [["tail"].into(), [("stdin", "test")].into()],
        "test",
    )]
    #[case::binary(
        [["echo", "-e", "\\xc3\\x28"].into(), [("decode", "binary")].into()],
        Value::buffer(b"\xc3\x28\n"),
    )]
    #[case::binary_from_text(
        // Strings can be decoded as binary
        [["echo", "hi"].into(), [("decode", "binary")].into()],
        Value::buffer(b"hi\n"),
    )]
    #[tokio::test]
    async fn test_command(
        #[case] arguments: impl IntoIterator<Item = Value>,
        #[case] expected_output: impl Into<Value>,
    ) {
        let output = call_fn("command", arguments, context()).await.unwrap();
        assert_eq!(output, expected_output.into());
    }

    #[rstest]
    #[case::empty([[""; 0].into()], "Command must have at least one element")]
    #[case::unknown(
        [["fake_command"].into()],
        "Executing command `fake_command`",
    )]
    #[case::binary(
        // Binary output with default text decoding is an error
        [["echo", "-e", "\\xc3\\x28"].into(), [("decode", "text")].into()],
        "invalid utf-8 sequence",
    )]
    #[tokio::test]
    async fn test_command_error(
        #[case] arguments: impl IntoIterator<Item = Value>,
        #[case] expected_error: &str,
    ) {
        let result = call_fn("command", arguments, context()).await;
        assert_err!(result, expected_error);
    }

    #[rstest]
    #[case::env(["TEST".into()], "test")]
    #[case::unknown(["TEST_UNKNOWN".into()], "")]
    #[tokio::test]
    async fn test_env(
        #[case] arguments: impl IntoIterator<Item = Value>,
        #[case] expected_output: impl Into<Value>,
    ) {
        let _guard = env_lock::lock_env([("TEST", Some("test"))]);
        let output = call_fn("env", arguments, context()).await.unwrap();
        assert_eq!(output, expected_output.into());
    }

    #[rstest]
    #[case::text("test.txt", [], "hello!")]
    #[case::bytes(
        "binary.bin",
        [[("decode", "binary")].into()],
        Value::buffer(b"\xc3\x28\n"),
    )]
    #[tokio::test]
    async fn test_file(
        temp_dir: TempDir,
        #[case] file_name: &str,
        #[case] arguments: impl IntoIterator<Item = Value>,
        #[case] expected_output: impl Into<Value>,
    ) {
        fs::write(&temp_dir.join("test.txt"), "hello!")
            .await
            .unwrap();
        fs::write(&temp_dir.join("binary.bin"), b"\xc3\x28\n")
            .await
            .unwrap();
        let path: Value = temp_dir.join(file_name).to_string_lossy().into();
        let arguments = iter::once(path).chain(arguments);
        let output = call_fn("file", arguments, context()).await.unwrap();
        assert_eq!(output, expected_output.into());
    }

    #[rstest]
    #[case::missing("unknown.txt", [], "No such file or directory")]
    #[case::binary(
        // Binary output with default text decoding is an error
        "binary.bin",
        [[("decode", "text")].into()],
        "invalid utf-8 sequence",
    )]
    #[tokio::test]
    async fn test_file_error(
        temp_dir: TempDir,
        #[case] file_name: &str,
        #[case] arguments: impl IntoIterator<Item = Value>,
        #[case] expected_error: &str,
    ) {
        fs::write(&temp_dir.join("binary.bin"), b"\xc3\x28\n")
            .await
            .unwrap();
        let path: Value = temp_dir.join(file_name).to_string_lossy().into();
        let arguments = iter::once(path).chain(arguments);
        let result = call_fn("file", arguments, context()).await;
        assert_err!(result, expected_error);
    }

    #[rstest]
    #[case::base(["$.data".into(), [("data", "hi")].into()], "hi")]
    #[case::mode_auto_one(
        ["$[*]".into(), ["a"].into(), [("mode", "auto")].into()],
        "a",
    )]
    #[case::mode_auto_many(
        ["$[*]".into(), ["a", "b"].into(), [("mode", "auto")].into()],
        ["a", "b"],
    )]
    #[case::mode_single_one(
        ["$[*]".into(), ["a"].into(), [("mode", "single")].into()],
        "a",
    )]
    #[case::mode_array_zero(
        ["$[*]".into(), [""; 0].into(), [("mode", "array")].into()],
        [""; 0],
    )]
    #[case::mode_array_one(
        ["$[*]".into(), ["a"].into(), [("mode", "array")].into()],
        ["a"],
    )]
    #[case::mode_array_many(
        ["$[*]".into(), ["a", "b"].into(), [("mode", "array")].into()],
        ["a", "b"],
    )]
    #[tokio::test]
    async fn test_json_path(
        #[case] arguments: impl IntoIterator<Item = Value>,
        #[case] expected_output: impl Into<Value>,
    ) {
        let output = call_fn("jsonPath", arguments, context()).await.unwrap();
        assert_eq!(output, expected_output.into());
    }

    #[rstest]
    #[case::invalid_query(
        ["w".into(), [("data", "hi")].into()],
        "Error converting argument 0",
    )]
    #[case::invalid_mode(
        ["$.data".into(), [("data", "hi")].into(), [("mode", "bad")].into()],
        "unknown variant `bad`",
    )]
    #[case::mode_auto_zero(
        ["$[*]".into(), [""; 0].into(), [("mode", "auto")].into()],
        "No results from JSONPath query `$[*]`",
    )]
    #[case::mode_single_zero(
        ["$[*]".into(), [""; 0].into(), [("mode", "single")].into()],
        "Expected exactly one result from JSONPath query `$[*]`",
    )]
    #[case::mode_single_many(
        ["$[*]".into(), ["a", "b"].into(), [("mode", "single")].into()],
        "Expected exactly one result from JSONPath query `$[*]`",
    )]
    #[tokio::test]
    async fn test_json_path_error(
        #[case] arguments: impl IntoIterator<Item = Value>,
        #[case] expected_error: &str,
    ) {
        let result = call_fn("jsonPath", arguments, context()).await;
        assert_err!(result, expected_error);
    }

    #[rstest]
    #[case::base(["field1".into()], "value1")]
    #[case::unknown(["fieldUnknown".into()], Value::Undefined)]
    // TODO test nested render
    #[tokio::test]
    async fn test_profile(
        #[case] arguments: impl IntoIterator<Item = Value>,
        #[case] expected_output: impl Into<Value>,
    ) {
        let profile = Profile {
            data: indexmap! { "field1".into() => "value1".into() },
            ..Profile::factory(())
        };
        let profile_id = profile.id.clone();
        let collection = Collection {
            profiles: by_id([profile]),
            recipes: Default::default(),
        };
        let context = RenderContext {
            collection: collection.into(),
            selected_profile: Some(profile_id),
            ..context()
        };
        let output = call_fn("profile", arguments, context).await.unwrap();
        assert_eq!(output, expected_output.into());
    }

    /// When the same profile field is accessed twice in the same process, we
    /// should only compute it once
    #[tokio::test]
    async fn test_profile_dedupe() {
        let profile = Profile {
            data: indexmap! {
                // TODO make this a prompt
                "field1".into() => "value1".into(),
            },
            ..Profile::factory(())
        };
        let profile_id = profile.id.clone();
        let collection = Collection {
            profiles: by_id([profile]),
            recipes: Default::default(),
        };
        let context = RenderContext {
            collection: collection.into(),
            selected_profile: Some(profile_id),
            prompter: Box::new(TestPrompter::new(["hello!"])),
            ..context()
        };
        let (process, f) = get_fn("profile", context);
        let output = task::spawn_blocking(move || {
            [
                // This will trigger the prompt
                process.call(&f, vec!["field1".into()]).unwrap(),
                // This one won't re-prompt. If it did, it would fail
                process.call(&f, vec!["field1".into()]).unwrap(),
            ]
        })
        .await
        .unwrap();
        assert_eq!(output, ["hello!".into(), "hello!".into()]);
    }

    #[rstest]
    #[case::nested_error([], "TODO")]
    #[tokio::test]
    async fn test_profile_error(
        #[case] arguments: impl IntoIterator<Item = Value>,
        #[case] expected_error: &str,
    ) {
        let result = call_fn("profile", arguments, context()).await;
        assert_err!(result, expected_error);
    }

    #[rstest]
    #[case::base(["test"], [], "test")]
    // We have no way to actually test that the message appears, but we can
    // at least test that it doesn't trigger an error
    #[case::message(["test"], [[("message", "Gimme that!")].into()], "test")]
    #[case::default([], [[("default", "default")].into()], "default")]
    #[case::sensitive(["test"], [[("sensitive", true)].into()], "••••")]
    #[tokio::test]
    async fn test_prompt(
        #[case] responses: impl IntoIterator<Item = &str>,
        #[case] arguments: impl IntoIterator<Item = Value>,
        #[case] expected_output: impl Into<Value>,
    ) {
        let context = RenderContext {
            prompter: Box::new(TestPrompter::new(responses)),
            show_sensitive: false,
            ..context()
        };
        let output = call_fn("prompt", arguments, context).await.unwrap();
        assert_eq!(output, expected_output.into());
    }

    #[rstest]
    #[tokio::test]
    async fn test_prompt_error() {
        let context = RenderContext {
            prompter: Box::new(TestPrompter::new([""; 0])),
            ..context()
        };
        let result = call_fn("prompt", [], context).await;
        assert_err!(result, "No reply from prompt/select");
    }

    #[rstest]
    #[case::base(["text".into()], "hello")]
    #[case::binary(
        ["binary".into(), [("decode", "binary")].into()],
        Value::buffer(b"\xc3\x28"),
    )]
    // TODO test triggering
    #[tokio::test]
    async fn test_response(
        #[case] arguments: impl IntoIterator<Item = Value>,
        #[case] expected_output: impl Into<Value>,
    ) {
        let database = CollectionDatabase::factory(());
        let request_text = RequestRecord::factory((None, "text".into()));
        let response_text = ResponseRecord {
            id: request_text.id,
            body: ResponseBody::new("hello".as_bytes().into()),
            ..ResponseRecord::factory(())
        };
        database
            .insert_exchange(&Exchange::factory((request_text, response_text)))
            .unwrap();
        let request_binary = RequestRecord::factory((None, "binary".into()));
        let response_binary = ResponseRecord {
            id: request_binary.id,
            body: ResponseBody::new(b"\xc3\x28".as_slice().into()),
            ..ResponseRecord::factory(())
        };
        database
            .insert_exchange(&Exchange::factory((
                request_binary,
                response_binary,
            )))
            .unwrap();
        let context = RenderContext {
            http_provider: Box::new(TestHttpProvider::new(database, None)),
            ..context()
        };

        let output = call_fn("response", arguments, context).await.unwrap();
        assert_eq!(output, expected_output.into());
    }

    #[rstest]
    // Binary output with default text decoding is an error
    #[case::binary(
        ["binary".into(), [("decode", "text")].into()],
        "invalid utf-8 sequence",
    )]
    #[case::trigger_never(
        ["missing".into(), [("trigger", "never")].into()],
        "No response available",
    )]
    #[case::trigger_not_allowed(
        ["binary".into(), [("trigger", "always")].into()],
        "Triggered request execution not allowed in this context",
    )]
    #[tokio::test]
    async fn test_response_error(
        #[case] arguments: impl IntoIterator<Item = Value>,
        #[case] expected_error: &str,
    ) {
        let database = CollectionDatabase::factory(());
        let request_binary = RequestRecord::factory((None, "binary".into()));
        let response_binary = ResponseRecord {
            id: request_binary.id,
            body: ResponseBody::new(b"\xc3\x28".as_slice().into()),
            ..ResponseRecord::factory(())
        };
        database
            .insert_exchange(&Exchange::factory((
                request_binary,
                response_binary,
            )))
            .unwrap();
        let context = RenderContext {
            http_provider: Box::new(TestHttpProvider::new(database, None)),
            ..context()
        };

        let result = call_fn("response", arguments, context).await;
        assert_err!(result, expected_error);
    }

    #[rstest]
    #[case::text(["r1".into(), "Text".into()], "test")]
    #[case::binary(
        ["r1".into(), "Binary".into(), [("decode", "binary")].into()],
        Value::buffer(b"\xc3\x28"),
    )]
    #[tokio::test]
    async fn test_response_header(
        #[case] arguments: impl IntoIterator<Item = Value>,
        #[case] expected_output: impl Into<Value>,
    ) {
        let database = CollectionDatabase::factory(());
        let request = RequestRecord::factory((None, "r1".into()));
        let response = ResponseRecord {
            id: request.id,
            headers: header_map([
                ("text", b"test".as_slice()),
                ("binary", b"\xc3\x28".as_slice()),
            ]),
            ..ResponseRecord::factory(())
        };
        database
            .insert_exchange(&Exchange::factory((request, response)))
            .unwrap();
        let context = RenderContext {
            http_provider: Box::new(TestHttpProvider::new(database, None)),
            ..context()
        };

        let output =
            call_fn("responseHeader", arguments, context).await.unwrap();
        assert_eq!(output, expected_output.into());
    }

    #[rstest]
    // Binary output with default text decoding is an error
    #[case::binary(
        ["r1".into(), "Binary".into(), [("decode", "text")].into()],
        "invalid utf-8 sequence",
    )]
    #[case::trigger_never(
        ["missing".into(), "Text".into(), [("trigger", "never")].into()],
        "No response available",
    )]
    #[case::trigger_not_allowed(
        ["r1".into(), "Text".into(), [("trigger", "always")].into()],
        "Triggered request execution not allowed in this context",
    )]
    #[tokio::test]
    async fn test_response_header_error(
        #[case] arguments: impl IntoIterator<Item = Value>,
        #[case] expected_error: &str,
    ) {
        let database = CollectionDatabase::factory(());
        let request = RequestRecord::factory((None, "r1".into()));
        let response = ResponseRecord {
            id: request.id,
            headers: header_map([
                ("text", b"test".as_slice()),
                ("binary", b"\xc3\x28".as_slice()),
            ]),
            ..ResponseRecord::factory(())
        };
        database
            .insert_exchange(&Exchange::factory((request, response)))
            .unwrap();
        let context = RenderContext {
            http_provider: Box::new(TestHttpProvider::new(database, None)),
            ..context()
        };

        let result = call_fn("responseHeader", arguments, context).await;
        assert_err!(result, expected_error);
    }

    #[rstest]
    #[case::base([["a", "b"].into()], "b")]
    // We have no way to actually test that the message appears, but we can
    // at least test that it doesn't trigger an error
    #[case::message(
        [["a", "b"].into(), [("message", "Gimme that!")].into()],
        "b",
    )]
    #[tokio::test]
    async fn test_select(
        #[case] arguments: impl IntoIterator<Item = Value>,
        #[case] expected_output: impl Into<Value>,
    ) {
        let context = RenderContext {
            prompter: Box::new(TestSelectPrompter::new([1])),
            ..context()
        };
        let output = call_fn("select", arguments, context).await.unwrap();
        assert_eq!(output, expected_output.into());
    }

    #[rstest]
    #[tokio::test]
    async fn test_select_error() {
        let context = RenderContext {
            prompter: Box::new(TestSelectPrompter::new([])),
            ..context()
        };
        let result = call_fn("select", [["a", "b"].into()], context).await;
        assert_err!(result, "No reply from prompt/select");
    }

    #[rstest]
    #[case::show(true, ["hello".into()], "hello")]
    #[case::hide(false, ["hello".into()], "•••••")]
    #[tokio::test]
    async fn test_sensitive(
        #[case] show_sensitive: bool,
        #[case] arguments: impl IntoIterator<Item = Value>,
        #[case] expected_output: impl Into<Value>,
    ) {
        let context = RenderContext {
            show_sensitive,
            ..context()
        };
        let output = call_fn("sensitive", arguments, context).await.unwrap();
        assert_eq!(output, expected_output.into());
    }

    /// Compile and run a process that exports the requested function. Return
    /// the compiled process and the generated function, to be called later.
    fn get_fn(name: &str, context: RenderContext) -> (Process, Function) {
        // Re-use the same engine for every test
        static ENGINE: LazyLock<Engine> = LazyLock::new(|| {
            let mut engine = Engine::new();
            register_module(&mut engine);
            engine
        });
        let mut process = ENGINE
            .compile(format!(
                "import {{ {name} }} from 'slumber'; export default {name};"
            ))
            .unwrap();
        process.set_app_data(context).unwrap();
        process.set_app_data(RenderState::default()).unwrap();
        let f = process
            .execute()
            .unwrap()
            .default
            .unwrap()
            .try_into_function()
            .unwrap();
        (process, f)
    }

    /// Call a function by name with some arguments
    async fn call_fn(
        name: &str,
        arguments: impl IntoIterator<Item = Value>,
        context: RenderContext,
    ) -> Result<Value, petitscript::Error> {
        let (process, f) = get_fn(name, context);
        let arguments = arguments.into_iter().collect();

        // Needs to be run in a background thread because it does blocking work
        task::spawn_blocking(move || process.call(&f, arguments))
            .await
            .unwrap()
    }

    /// Build a render context with some reasonable defaults
    fn context() -> RenderContext {
        RenderContext {
            collection: Collection::factory(()).into(),
            selected_profile: None,
            http_provider: Box::new(TestHttpProvider::new(
                CollectionDatabase::factory(()),
                None,
            )),
            overrides: Overrides::new(),
            prompter: Box::new(TestPrompter::new([""; 0])),
            show_sensitive: true,
        }
    }
}
