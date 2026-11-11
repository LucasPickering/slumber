//! Functions available in templates

use crate::{
    collection::RecipeId,
    render::{FunctionError, ProcessExt, SelectOption, TemplateContext},
};
use base64::{Engine, prelude::BASE64_STANDARD};
use bytes::Bytes;
use futures::{FutureExt, Stream, StreamExt, TryFutureExt, TryStreamExt};
use itertools::Itertools;
use petitscript::{Process, Value};
use serde::{Deserialize, de::IntoDeserializer};
use slumber_util::{TimeSpan, paths::expand_home};
use std::{
    env, fmt::Debug, io, path::PathBuf, process::Stdio, str::FromStr, sync::Arc,
};
use tokio::{
    fs::File,
    io::{AsyncRead, AsyncWriteExt},
    process::Command,
};
use tokio_util::io::ReaderStream;
use tracing::{Instrument, debug, debug_span};

// ===========================================================
// Documentation for these functions is generated automatically by an mdbook
// preprocessor in the doc_utils crate. Each doc comment must be YAML adhering
// to a specific schema. See template_functions.rs for the schema.
// ===========================================================

/// ```notrust
/// description: Encode or decode content to/from base64
/// tags: [string]
/// parameters:
///   value:
///     description: Value to encode or decode
///   decode:
///     description: Decode the input from base64 to its original value instead
///       of encoding it *to* base64
///     default: false
/// return: The encoded value (if `decode=false`) or decoded value
///     (if `decode=true`)
/// errors:
///   - If `decode=true` and `value` is not a valid base64 string
/// examples:
///   - input: base64("test")
///     output: '"dGVzdA=="'
///   - input: base64("dGVzdA==", { decode: true })
///     output: '"test"'
/// ```
pub fn base64(
    process: &Process,
    value: Bytes,
    Base64Kwargs { decode }: Base64Kwargs,
) -> Result<Bytes, FunctionError> {
    if decode {
        BASE64_STANDARD
            .decode(&value)
            .map(Bytes::from)
            .map_err(FunctionError::from)
    } else {
        Ok(BASE64_STANDARD.encode(&value).into())
    }
}
struct Base64Kwargs {
    decode: bool,
}

/// ```notrust
/// description: Run a command in a subprocess and return its stdout output.
///   Supports streaming of stdout.
/// tags: [input]
/// parameters:
///   command:
///     description: Command to run, in the form [program, arg1, arg2, ...]
///   cwd:
///     description: Directory to execute the subprocess in. The given path will
///         be resolved relative to the directory containing the collection file.
///     default: .
///   stdin:
///     description: Data to pipe to the subprocess's stdin
///     default: "b''"
/// return: Stdout output as bytes
/// errors:
///   - If the command fails to initialize (e.g. program unknown)
///   - If the subprocess exits with a non-zero status code
/// examples:
///   - input: command(["echo", "hello"])
///     output: "hello\n"
///   - input: command(["grep","1"], { stdin: "line 1\nline2" })
///     output: "line 1\n"
/// ```
pub fn command(
    process: &Process,
    command: Vec<String>,
    CommandKwargs { cwd, stdin }: CommandKwargs,
) -> Result<ValueStream, FunctionError> {
    /// Wrap an IO error
    fn io_error(
        program: &str,
        arguments: &[String],
        error: io::Error,
    ) -> FunctionError {
        FunctionError::CommandInit {
            program: program.to_owned(),
            arguments: arguments.to_owned(),
            error,
        }
    }

    let context = process.context()?;
    let cwd = context.root_dir.join(cwd.unwrap_or_default());
    let [program, arguments @ ..] = command.as_slice() else {
        return Err(FunctionError::CommandEmpty);
    };
    let program = program.clone();
    let arguments = arguments.to_owned();

    // We're going to defer command spawning *and* streaming. Streamed commands
    // shouldn't be spawned until the stream is actually resolved, to prevent
    // running large/slow commands in a preview.
    //
    // We construct a 3-stage stream:
    // - Spawn command
    // - Stream from stdout
    // - Check command status
    let span = debug_span!("command()", ?program, ?arguments);
    let span_ = span.clone(); // Clone so we can attach to the inner stream too
    let future = async move {
        // Spawn the command process
        debug!("Spawning");
        let mut child = Command::new(&program)
            .args(&arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .current_dir(cwd)
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| io_error(&program, &arguments, error))?;

        // Write the stdin to the process
        if let Some(stdin) = stdin {
            child
                .stdin
                .as_mut()
                .expect("Process missing stdin")
                .write_all(&stdin)
                .await
                .map_err(|error| io_error(&program, &arguments, error))?;
        }

        // We have to poll the process (via wait()) and stream from stdout
        // simultaneously. If we just stream from stdout, we never get any
        // output. If we try to wait() then stream from stdout, the stdout
        // buffer may fill up and the process will hang until it's drained. In
        // practice this means we'll poll in a background task, then stream
        // stdout until it's done.
        let stdout = child.stdout.take().expect("stdout not set for child");
        let handle = tokio::spawn(async move { child.wait().await });

        // After stdout is done, we'll check the status code of the process to
        // make sure it succeeded. This gets chained on to the end of
        // the stream
        let status_future = async move {
            let status_result = handle.await;
            debug!(?status_result, "Finished");
            let status = status_result
                .map_err(RenderError::other)? // Join error - task panicked
                // Command error
                .map_err(|error| io_error(&program, &arguments, error))?;
            if status.success() {
                // Since we're chaining onto the end of the output stream, we
                // need to emit empty bytes
                Ok(Bytes::new())
            } else {
                Err(FunctionError::CommandStatus {
                    program,
                    arguments,
                    status,
                }
                .into())
            }
        }
        .instrument(span_);
        Ok(reader_stream(stdout).chain(status_future.into_stream()))
    }
    .instrument(span);

    let stream = future.try_flatten_stream().boxed();

    Ok(ValueStream::Stream {
        source: StreamSource::Command { command },
        stream,
    })
}
struct CommandKwargs {
    cwd: Option<String>,
    stdin: Option<Bytes>,
}

/// ```notrust
/// description: Print a value to stdout, returning the same value. Useful for debugging templates.
/// parameters:
///   value:
///     description: The value to print and return
/// return: The same value that was passed in
/// examples:
///   - input: debug("hello")
///     output: "'hello'"
///     comment: Prints "hello"
///   - input: 'file("data.json") | debug() | jsonpath("$.data")'
///     output: '123'
///     comment: Contents of data.json will be printed
/// ```
pub fn debug(value: Value) -> Value {
    println!("{value:?}");
    value
}

/// ```notrust
/// description: Get the value of an environment variable, or `""` if not set
/// tags: [input]
/// parameters:
///   variable:
///     description: Name of the environment variable to read
///   default:
///     description: Value to return when the environment variable is not present
///     default: ""
/// return: Value of the environment variable or the provided default
/// examples:
///   - input: env("HOME")
///     output: "/home/username"
///   - input: env("NONEXISTENT")
///     output: ""
///   - input: env("NONEXISTENT", default="default")
///     output: "default"
/// ```
pub fn env(variable: String, EnvKwargs { default }: EnvKwargs) -> String {
    env::var(variable).unwrap_or(default)
}
struct EnvKwargs {
    default: String,
}

/// ```notrust
/// description: Load contents of a file. Output is bytes but can be used as a
///   string in most cases. Supports streaming for large/binary files.
/// tags: [input]
/// parameters:
///   path:
///     description: Path to the file to read, relative to the collection file (`slumber.yml`). A leading `~` will be expanded to $HOME.
/// return: File contents as bytes (may be a stream)
/// errors:
///   - If an I/O error occurs while opening the file (e.g. file missing)
/// examples:
///   - input: file("config.json")
///     output: Contents of config.json file
/// ```
pub fn file(process: &Process, (path,): (String,)) -> ValueStream {
    let context = process.context()?;
    let path = context.root_dir.join(expand_home(PathBuf::from(path)));
    let source = StreamSource::File { path: path.clone() };
    // Return the file as a stream. If streaming isn't available here, it will
    // be resolved immediately instead. If the file doesn't exist or any other
    // error occurs, the error will be deferred until the data is actually
    // streamed.
    let future = async move {
        let file = File::open(&path)
            .await
            .map_err(|error| FunctionError::File { path, error })?;
        Ok(reader_stream(file))
    };
    ValueStream::Stream {
        source,
        stream: future.try_flatten_stream().boxed(),
    }
}

/// ```notrust
/// description: Prompt the user to enter a text value
/// tags: [input]
/// parameters:
///   message:
///     description: Optional prompt message to display to the user
///     default: "''"
///   default:
///     description: Optional default value to pre-fill the input
///     default: "''"
///   sensitive:
///     description: Mask the input while typing. Also masks output in previews.
///     default: false
/// return: Entered string
/// errors:
///   - If the user doesn't give a response
/// examples:
///   - input: prompt()
///     output: "What do I put here? Help!!"
///   - input: prompt(message="Enter your name")
///     output: "Barry Barracuda"
///   - input: prompt(message="Password", sensitive=true)
///     output: "hunter2"
/// ```
pub async fn prompt(
    #[context] context: &TemplateContext,
    #[kwarg] message: Option<String>,
    #[kwarg] default: Option<String>,
    #[kwarg] sensitive: bool,
) -> Result<String, FunctionError> {
    let reply = context
        .prompter
        .prompt_text(message.unwrap_or_default(), default, sensitive)
        .await
        .ok_or(FunctionError::PromptNoReply)?;

    // If the input was sensitive, we should mask the output as well. This only
    // impacts previews as show_sensitive is enabled for request renders. This
    // means the only string that's actually impacted by this masking is the
    // static output string from the preview prompter, i.e. instead of showing
    // "<prompt>", we show "••••••••". It's "technically" right and plays well
    // in tests. Also it reminds users that a prompt is sensitive in the TUI
    if sensitive {
        Ok(mask_sensitive(context, reply))
    } else {
        Ok(reply)
    }
}

/// ```notrust
/// description: Load the most recent response body for the given recipe and
///   current profile
/// tags: [input]
/// parameters:
///   recipe_id:
///     description: ID (not name) of the recipe to load the response from
///   trigger:
///     description: When to execute the upstream request (never/no_history/always/Duration)
///     default: "never"
/// return: Most recent response body as bytes
/// errors:
///   - If `recipe` isn't in the collection
///   - If there is no request in history and `trigger='never'`
///   - If a request is triggered and failed
/// examples:
///   - input: response("login")
///     output: '{"token": "abc123"}'
///   - input: response("login", trigger="1h")
///     output: '{"token": "abc123"}'
/// ```
pub async fn response(
    #[context] context: &TemplateContext,
    recipe_id: RecipeId,
    #[kwarg] trigger: RequestTrigger,
) -> Result<Bytes, FunctionError> {
    let response = context.get_latest_response(&recipe_id, trigger).await?;
    let body = match Arc::try_unwrap(response) {
        Ok(response) => response.body,
        Err(response) => response.body.clone(),
    };
    Ok(body.into_bytes())
}

/// ```notrust
/// description: Load a header value from the most recent response for a recipe
///   and the current profile
/// tags: [input]
/// parameters:
///   recipe_id:
///     description: ID (not name) of the recipe to load the response from
///   header:
///     description: Name of the header to extract (case-insensitive)
///   trigger:
///     description: When to execute the upstream request vs using cached response
///     default: "never"
/// return: Header value as bytes
/// errors:
///   - If `recipe` isn't in the collection
///   - If there is no request in history and `trigger='never'`
///   - If a request is triggered and failed
///   - If the header is missing
/// examples:
///   - input: response_header("get_rate_limit", "X-Rate-Limit", trigger="5m")
///     output: "100"
/// ```
pub async fn response_header(
    #[context] context: &TemplateContext,
    recipe_id: RecipeId,
    header: String,
    #[kwarg] trigger: RequestTrigger,
) -> Result<Bytes, FunctionError> {
    let response = context.get_latest_response(&recipe_id, trigger).await?;
    // Only clone the header value if necessary
    let header_value = match Arc::try_unwrap(response) {
        Ok(mut response) => response.headers.remove(&header),
        Err(response) => response.headers.get(&header).cloned(),
    }
    .ok_or_else(|| FunctionError::ResponseMissingHeader { header })?;
    // HeaderValue doesn't expose any way to move its bytes out so we must clone
    // https://github.com/hyperium/http/issues/661
    Ok(header_value.as_bytes().to_vec().into())
}

/// ```notrust
/// description: Ask the user to select a value from a list
/// tags: [input]
/// parameters:
///   options:
///     description: List of options to choose from. Each option can be either a string or an object with "label" and "value".
///   message:
///     description: Descriptive message to display to the user
///     default: ""
/// return: The selected value
/// errors:
///   - If `options` is empty
///   - If the user doesn't give a response
/// examples:
///   - input: select(["dev", "staging", "prod"])
///     output: "dev"
///   - input: select(["GET", "POST","PUT"], message="HTTP method")
///     output: "POST"
///   - input: select([{"label":"Sam","value":1},{"label":"Mike","value":2}])
///     output: 2
/// ```
pub async fn select(
    #[context] context: &TemplateContext,
    options: Vec<SelectOption>,
    #[kwarg] message: Option<String>,
) -> Result<Value, FunctionError> {
    // If there are no options, we can't show anything meaningful to the user.
    // We *could* just return an empty string but that may be confusing.
    // Something probably went wrong upstream so return an error.
    if options.is_empty() {
        return Err(FunctionError::SelectNoOptions);
    }

    context
        .prompter
        .prompt_select(message.unwrap_or_default(), options)
        .await
        .ok_or(FunctionError::PromptNoReply)
}

/// A select option can be given as an object of `{value, label}` or a single
/// string (or other scalar value).
impl TryFromValue for SelectOption {
    fn try_from_value(value: Value) -> Result<Self, WithValue<ValueError>> {
        match value {
            Value::Object(ref map) => {
                // Use serde to deserialize the object into a struct
                Self::deserialize(map.into_deserializer())
                    .map_err(|error| WithValue::new(value, error))
            }
            value => Ok(Self {
                label: value.clone().try_into_string()?,
                value,
            }),
        }
    }
}

/// ```notrust
/// description: Mark a value as sensitive, masking it in template previews.
///   No impact on requests sent.
/// tags: [string]
/// parameters:
///   value:
///     description: String to mask
/// return: Masked string in preview, input string when building requests
/// examples:
///   - input: sensitive("hunter2")
///     output: "•••••••"
/// ```
pub fn sensitive(
    #[context] context: &TemplateContext,
    value: String,
) -> String {
    mask_sensitive(context, value)
}

fn mask_sensitive(context: &TemplateContext, value: String) -> String {
    if context.show_sensitive {
        value
    } else {
        "•".repeat(value.chars().count())
    }
}

/// Define when a recipe with a chained request should auto-execute the
/// dependency request.
#[derive(Copy, Clone, Debug, Default)]
pub enum RequestTrigger {
    /// Never trigger the request. This is the default because upstream
    /// requests could be mutating, so we want the user to explicitly opt into
    /// automatic execution.
    #[default]
    Never,
    /// Trigger the request if there is none in history
    NoHistory,
    /// Trigger the request if the last response is older than some
    /// duration (or there is none in history)
    Expire { duration: TimeSpan },
    /// Trigger the request every time the dependent request is rendered
    Always,
}

/// Parse a request trigger from a string. Unit variants are assigned a static
/// string, and anything else is treated as an expire duration.
impl FromStr for RequestTrigger {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            // If you add a case here, update the expecting string too
            "never" => Ok(Self::Never),
            "no_history" => Ok(Self::NoHistory),
            "always" => Ok(Self::Always),
            // Anything else is parsed as a duration
            _ => {
                let duration = s.parse::<TimeSpan>().map_err(|_| {
                    "Expected \"never\", \"no_history\", \"always\", or a \
                    duration string such as \"1h\" \
                    (units are \"s\", \"m\", \"h\", or \"d\")"
                })?;
                Ok(Self::Expire { duration })
            }
        }
    }
}

impl_try_from_value_str!(RequestTrigger);

/// Trim whitespace from a string
#[derive(Copy, Clone, Debug, Default)]
pub enum TrimMode {
    /// Trim the start of the output
    Start,
    /// Trim the end of the output
    End,
    /// Trim the start and end of the output
    #[default]
    Both,
}

// Manual implementation provides the best error messages
impl FromStr for TrimMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "start" => Ok(Self::Start),
            "end" => Ok(Self::End),
            "both" => Ok(Self::Both),
            _ => Err(format!(
                "Invalid mode `{s}`; must be `start`, `end`, or `both`"
            )),
        }
    }
}

impl_try_from_value_str!(TrimMode);

impl TryFromValue for RecipeId {
    fn try_from_value(value: Value) -> Result<Self, WithValue<ValueError>> {
        String::try_from_value(value).map(RecipeId::from)
    }
}

/// Create a stream from an `AsyncRead` value
fn reader_stream(
    reader: impl AsyncRead,
) -> impl Stream<Item = Result<Bytes, RenderError>> {
    ReaderStream::new(reader).map_err(RenderError::other)
}

// There are no unit tests for these functions. Instead we use integration-ish
// tests in render/tests.rs because they're able to test input/output conversion
// as well, which is a core part of the function operation.
