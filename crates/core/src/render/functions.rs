//! Functions available in templates

use crate::{
    collection::RecipeId,
    render::{FunctionError, Prompt, Select, TemplateContext},
};
use bytes::Bytes;
use derive_more::FromStr;
use serde::{Deserialize, de::value::SeqDeserializer};
use serde_json_path::NodeList;
use slumber_macros::template;
use slumber_template::{RenderError, TryFromValue, impl_try_from_value_str};
use slumber_util::TimeSpan;
use std::{env, fmt::Debug, process::Stdio, sync::Arc};
use tokio::{fs, io::AsyncWriteExt, process::Command, sync::oneshot};
use tracing::{debug, debug_span};

// ===========================================================
// Documentation for these functions is generated automatically by an mdbook
// preprocessor in the doc_utils crate. The generator will generally enforce
// that each function has sufficient documentation on it. That said...
//
// DOC COMMENTS ON TEMPLATE FUNCTIONS SHOULD BE WRITTEN FOR THE USER. They are
// public-facing!!
// ===========================================================

/// Run a command in a subprocess and return its stdout output. While the output
/// type is `bytes`, [in most cases you can use it interchangeably as a
/// string](../user_guide/templates/values.md#bytes-vs-string).
///
/// **Parameters**
///
/// - `command`: Command to run, in the form `[program, arg1, arg2, ...]`
/// - `stdin`: Data to pipe to the subprocess's stdin
///
/// **Examples**
///
/// ```sh
/// {{ command(["echo", "hello"]) }} => "hello\n"
/// {{ command(["grep", "1"], stdin="line 1\nline2") }} => "line 1\n"
/// ```
///
/// > `command` is commonly paired with [`trim`](#trim) to remove trailing
/// newlines from command output: `{{ command(["echo", "hello"]) | trim() }}`
#[template(TemplateContext)]
pub async fn command(
    command: Vec<String>,
    #[kwarg] stdin: Option<String>,
) -> Result<Bytes, FunctionError> {
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

    Ok(output.stdout.into())
}

/// Print a value to stdout, returning the same value. This isn't very useful
/// in the TUI because stdout gets clobbered, but it can be helpful for
/// debugging templates with the CLI.
///
/// **Parameters**
///
/// - `value`: The value to print and return
///
/// **Examples**
///
/// ```sh
/// {{ debug("hello") }} => "hello" (also prints "hello" to stdout)
/// {{ file("data.json") | debug() | jsonpath("$.data") }} => Extract data field and print intermediate result
/// ```
#[template(TemplateContext)]
pub fn debug(value: slumber_template::Value) -> slumber_template::Value {
    println!("{value:?}");
    value
}

/// Get the value of an environment variable, or `null` if not set.
///
/// **Parameters**
///
/// - `variable`: Name of the environment variable to read
///
/// **Examples**
///
/// ```sh
/// {{ env("HOME") }} => "/home/username"
/// {{ env("NONEXISTENT") }} => null
/// ```
#[template(TemplateContext)]
pub fn env(variable: String) -> Option<String> {
    env::var(variable).ok()
}

/// Load contents of a file. While the output type is `bytes`,
/// [in most cases you can use it interchangeably as a
/// string](../user_guide/templates/values.md#bytes-vs-string). `bytes`
/// support means you can also use this to load binary files such as images,
/// which can be useful for request bodies.
///
/// **Parameters**
///
/// - `path`: Path to the file to read, relative to the collection file
///   (`slumber.yml`) in use
///
/// **Examples**
///
/// ```sh
/// {{ file("config.json") }} => Contents of config.json file
/// ```
#[template(TemplateContext)]
pub async fn file(path: String) -> Result<Bytes, FunctionError> {
    let bytes = fs::read(&path).await.map_err(|error| FunctionError::File {
        path: path.into(),
        error,
    })?;
    Ok(bytes.into())
}

/// Wrapper for [serde_json_path::JsonPath] to enable implementing
/// [TryFromValue]
#[derive(Debug, FromStr)]
pub struct JsonPath(serde_json_path::JsonPath);

impl_try_from_value_str!(JsonPath);

/// Control how a JSONPath selector returns 0 vs 1 vs 2+ results
#[derive(Copy, Clone, Debug, Default)]
#[cfg_attr(any(test, feature = "test"), derive(PartialEq))]
pub enum JsonPathMode {
    /// 0 - Error
    /// 1 - Single result
    /// 2 - Array of values
    #[default]
    Auto,
    /// 0 - Error
    /// 1 - Single result
    /// 2 - Error
    Single,
    /// 0 - Array of values
    /// 1 - Array of values
    /// 2 - Array of values
    Array,
}

// Manual implementation provides the best error messages
impl FromStr for JsonPathMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "auto" => Ok(Self::Auto),
            "single" => Ok(Self::Single),
            "array" => Ok(Self::Array),
            _ => Err(format!(
                "Invalid mode `{s}`; must be `array`, `single`, or `auto`"
            )),
        }
    }
}

impl_try_from_value_str!(JsonPathMode);

/// Transform a JSON value using a JSONPath query. See
/// [JSONPath specification](https://datatracker.ietf.org/doc/html/rfc9535) or
/// [jsonpath.com](https://jsonpath.com/) for query syntax.
///
/// This function is most useful when used after a data-providing function such
/// as [`file`](#file) or [`response`](#response).
///
/// **Parameters**
///
/// - `value`: JSON value to query (typically piped in)
/// - `query`: JSONPath query string
/// - `mode`: How to handle multiple results (see table below; default:
///   `"auto"`)
///
/// An explanation of `mode` using this object as an example:
///
/// ```json
/// [{ "name": "Apple" }, { "name": "Kiwi" }, { "name": "Mango" }]
/// ```
///
/// | Mode     | Description                                                                       | `$.id` | `$[0].name` | `$[*].name`                  |
/// | -------- | --------------------------------------------------------------------------------- | ------ | ----------- | ---------------------------- |
/// | `auto`   | If query returns a single value, use it. If it returns multiple, use a JSON array | Error  | `Apple`     | `["Apple", "Kiwi", "Mango"]` |
/// | `single` | If a query returns a single value, use it. Otherwise, error.                      | Error  | `Apple`     | Error                        |
/// | `array`  | Return results as an array, regardless of count.                                  | `[]`   | `["Apple"]` | `["Apple", "Kiwi", "Mango"]` |
///
///
/// **Examples**
///
/// ```sh
/// {{ response('get_user') | jsonpath("$.first_name") }} => "Alice"
/// ```
#[template(TemplateContext)]
pub fn jsonpath(
    // Value first so it can be piped in
    value: serde_json::Value,
    query: JsonPath,
    #[kwarg] mode: JsonPathMode,
) -> Result<slumber_template::Value, FunctionError> {
    fn node_list_to_value(node_list: NodeList) -> slumber_template::Value {
        slumber_template::Value::deserialize(SeqDeserializer::new(
            node_list.into_iter(),
        ))
        // This conversion is infallible because JSON is a subset of Value and
        // the NodeList produces an array of JSON values
        .unwrap()
    }

    let query = query.0;
    let node_list = query.query(&value);

    // Convert the node list to a template value based on mode
    match mode {
        JsonPathMode::Auto => match node_list.len() {
            0 => Err(FunctionError::JsonPathNoResults { query }),
            1 => {
                let json = node_list.exactly_one().unwrap().clone();
                Ok(slumber_template::Value::from_json(json))
            }
            2.. => Ok(node_list_to_value(node_list)),
        },
        JsonPathMode::Single => {
            let json = node_list
                .exactly_one()
                .map_err(|_| FunctionError::JsonPathExactlyOne {
                    query,
                    actual_count: node_list.len(),
                })?
                .clone();
            Ok(slumber_template::Value::from_json(json))
        }
        JsonPathMode::Array => Ok(node_list_to_value(node_list)),
    }
}

/// Prompt the user to enter a text value.
///
/// **Parameters**
///
/// - `message`: Optional prompt message to display to the user
/// - `default`: Optional default value to pre-fill the input
/// - `sensitive`: Mask the input while typing. Unlike the
///   [`sensitive`](#sensitive) function, which masks *output* values, this flag
///   enables masking on the input *and* output. This means it's redundant to
///   combine `sensitive` with `prompt`.
///
/// **Examples**
///
/// ```sh
/// # Prompt with no message. User may be confused!
/// {{ prompt() }} => "What do I put here? Help!!"
/// # Prompts with custom message
/// {{ prompt(message="Enter your name") }} => "Barry Barracuda"
/// # Mask input while the user types
/// {{ prompt(message="Password", sensitive=true) }} => "hunter2"
/// ```
#[template(TemplateContext)]
pub async fn prompt(
    #[context] context: &TemplateContext,
    #[kwarg] message: Option<String>,
    #[kwarg] default: Option<String>,
    #[kwarg] sensitive: bool,
) -> Result<String, FunctionError> {
    let (tx, rx) = oneshot::channel();
    context.prompter.prompt(Prompt {
        message: message.unwrap_or_default(),
        default,
        sensitive,
        channel: tx.into(),
    });
    let output = rx.await.map_err(|_| FunctionError::PromptNoReply)?;

    // If the input was sensitive, we should mask the output as well. This only
    // impacts previews as show_sensitive is enabled for request renders. This
    // means the only string that's actually impacted by this masking is the
    // static output string from the preview prompter, but it's "technically"
    // right and plays well in tests. Also it reminds users that a prompt is
    // sensitive in the TUI :)
    if sensitive {
        Ok(mask_sensitive(context, output))
    } else {
        Ok(output)
    }
}

/// Load the most recent response body for the given recipe and current profile.
/// While the output type is `bytes`, [in most cases you can use it
/// interchangeably as a
/// string](../user_guide/templates/values.md#bytes-vs-string).
///
/// **Parameters**
///
/// - `recipe_id`: ID of the recipe to load the response from
/// - `trigger`: When to execute the upstream request
///
/// An explanation of `trigger`:
///
/// | Value          | Description                                                                                                            |
/// | -------------- | ---------------------------------------------------------------------------------------------------------------------- |
/// | `"never"`      | Never trigger. The most recent response in history for the upstream recipe will always be used; error if there is none |
/// | `"no_history"` | Trigger only if there is no response in history for the upstream recipe                                                |
/// | `"always"`     | Always execute the upstream request                                                                                    |
/// | `Duration`     | Trigger if the most recent response for the upstream recipe is older than some duration, or there is none              |
///
/// `Duration` is a `string` in the format `<quantity><unit>...`, e.g. `"3h"`.
/// Supported units are:
///
/// - `s` (seconds)
/// - `m` (minutes)
/// - `h` (hours)
/// - `d` (days)
///
/// Multiple units can be combined:
///
/// - `"10h5m"`: 10 hours and 5 minutes
/// - `"3d2s"`: 3 days and 2 seconds
///
/// **Examples**
///
/// ```sh
/// # Use the most recent response body. Error if there are no responses in history
/// {{ response("login") }} => {"token": "abc123"}
/// # Re-execute if older than 1 hour
/// {{ response("login", trigger="1h") }} => {"token": "abc123"}
/// # Combine with jsonpath for data extraction
/// {{ response("login") | jsonpath("$.token") }} => "abc123"
/// ```
///
/// > `response` is commonly combined with [`jsonpath`](#jsonpath) to extract
/// > data from JSON responses
#[template(TemplateContext)]
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

/// Load a header value from the most recent response for a recipe and the
/// current profile. While the output type is `bytes`,
/// [in most cases you can use it interchangeably as a
/// string](../user_guide/templates/values.md#bytes-vs-string).
///
/// **Parameters**
///
/// - `recipe_id`: ID of the recipe to load the response from
/// - `header`: Name of the header to extract (case-insensitive)
/// - `trigger`: When to execute the upstream request vs using the cached
///   response; [see `response`](#response)
///
/// **Examples**
///
/// ```sh
/// # Fetch current rate limit, refreshed if older than 5 minutes
/// {{ response_header("get_rate_limit", "X-Rate-Limit", trigger="5m") }} => Value of X-Rate-Limit response header
/// ```
#[template(TemplateContext)]
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

/// Ask the user to select a value from a list.
///
/// **Parameters**
///
/// - `options`: List of options to choose from
/// - `message`: Descriptive message to display to the user
///
/// **Examples**
///
/// ```sh
/// {{ select(["dev", "staging", "prod"]) }} => "dev"
/// # Custom prompt message
/// {{ select(["GET", "POST", "PUT"], message="HTTP method") }} => "POST"
/// ```
#[template(TemplateContext)]
pub async fn select(
    #[context] context: &TemplateContext,
    options: Vec<String>,
    #[kwarg] message: Option<String>,
) -> Result<String, FunctionError> {
    let (tx, rx) = oneshot::channel();
    context.prompter.select(Select {
        message: message.unwrap_or_default(),
        options,
        channel: tx.into(),
    });
    let output = rx.await.map_err(|_| FunctionError::PromptNoReply)?;
    Ok(output)
}

/// Mark a value as sensitive, masking it in template previews. This has no
/// impact on how requests are actually sent, it only prevents sensitive values
/// such as passwords from being displayed in the recipe preview.
///
/// **Parameters**
///
/// - `value`: String to mask
///
/// **Examples**
///
/// ```sh
/// {{ sensitive("hunter2") }} => "•••••••" (in preview)
/// ```
#[template(TemplateContext)]
pub fn sensitive(
    #[context] context: &TemplateContext,
    value: String,
) -> String {
    mask_sensitive(context, value)
}

/// Trim whitespace from the beginning and/or end of a string.
///
/// **Parameters**
///
/// - `value`: String to trim (typically piped in from a previous function with
///   `|`)
/// - `mode` (default: `"both"`): Section of the string to trim
///
/// **Examples**
///
/// ```sh
/// {{ trim("  hello  ") }} => "hello"
/// {{ trim("  hello  ", mode="start") }} => "hello  "
/// # Remove trailing newline from command output
/// {{ command(["echo", "hello"]) | trim() }} => "hello"
/// ```
#[template(TemplateContext)]
pub fn trim(value: String, #[kwarg] mode: TrimMode) -> String {
    match mode {
        TrimMode::Start => value.trim_start().to_string(),
        TrimMode::End => value.trim_end().to_string(),
        TrimMode::Both => value.trim().to_string(),
    }
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
                    "\"never\", \"no_history\", \"always\", or a duration \
                    string such as \"1h\"; duration units are `s`, `m`, `h`, or `d`"
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
    fn try_from_value(
        value: slumber_template::Value,
    ) -> Result<Self, slumber_template::RenderError> {
        String::try_from_value(value).map(RecipeId::from)
    }
}
