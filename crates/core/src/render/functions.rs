//! Functions available in templates

use crate::{
    collection::RecipeId,
    render::{
        FunctionError, Prompt, Select, SelectOption, SingleRenderContext,
    },
};
use base64::{Engine, prelude::BASE64_STANDARD};
use bytes::Bytes;
use derive_more::FromStr;
use futures::{FutureExt, Stream, StreamExt, TryFutureExt, TryStreamExt};
use itertools::Itertools;
use serde::{Deserialize, de::IntoDeserializer};
use slumber_macros::template;
use slumber_template::{
    Expected, LazyValue, RenderError, StreamSource, TryFromValue, Value,
    ValueError, WithValue, impl_try_from_value_str,
};
use slumber_util::{TimeSpan, paths::expand_home};
use std::{env, fmt::Debug, io, path::PathBuf, process::Stdio, sync::Arc};
use tokio::{
    fs::File,
    io::{AsyncRead, AsyncWriteExt},
    process::Command,
    sync::oneshot,
};
use tokio_util::io::ReaderStream;
use tracing::{Instrument, debug, debug_span};

// ===========================================================
// Documentation for these functions is generated automatically by an mdbook
// preprocessor in the doc_utils crate. The generator will generally enforce
// that each function has sufficient documentation on it. That said...
//
// DOC COMMENTS ON TEMPLATE FUNCTIONS SHOULD BE WRITTEN FOR THE USER. They are
// public-facing!!
// ===========================================================

/// Encode or decode content to/from base64.
///
/// **Parameters**
///
/// - `decode`: Decode the input from base64 to its original value instead of
///   encoding it *to* base64
///
/// **Errors**
///
/// - If `decode=true` and the string is not valid base64
///
/// **Examples**
///
/// ```sh
/// {{ base64("test") }} => "dGVzdA=="
/// {{ base64("dGVzdA==", decode=true) }} => "test"
/// ```
#[template]
pub fn base64(
    value: Bytes,
    #[kwarg] decode: bool,
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

/// Convert a value to a boolean. Empty values such as `0`, `""` or `[]`
/// convert to `false`. Anything else converts to `true`.
///
/// **Parameters**
///
/// - `value`: Value to convert
///
/// **Examples**
///
/// ```sh
/// {{ boolean(null) }} => false
/// {{ boolean(0) }} => false
/// {{ boolean(1) }} => true
/// {{ boolean('') }} => false
/// {{ boolean('0') }} => true
/// {{ boolean([]) }} => false
/// {{ boolean([0]) }} => true
/// ```
#[template]
pub fn boolean(value: Value) -> bool {
    value.to_bool()
}

/// Run a command in a subprocess and return its stdout output. While the output
/// type is `bytes`, [in most cases you can use it interchangeably as a
/// string](../user_guide/templates/values.md#bytes-vs-string).
///
/// This function supports [streaming](../user_guide/streaming.html).
///
/// **Parameters**
///
/// - `command`: Command to run, in the form `[program, arg1, arg2, ...]`
/// - `cwd`: Directory to execute the subprocess in. Defaults to the directory
///   containing the collection file. For example, if the collection is
///   `/data/slumber.yml`, the subprocess will execute in `/data` regardless of
///   where Slumber is invoked from. The given path will be resolved relative to
///   that default.
/// - `stdin`: Data to pipe to the subprocess's stdin
///
/// **Errors**
///
/// - If the command fails to initialize (e.g. the program is unknown)
/// - If the subprocess exits with a non-zero status code
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
#[template]
pub fn command(
    #[context] context: &SingleRenderContext<'_>,
    command: Vec<String>,
    #[kwarg] cwd: Option<String>,
    #[kwarg] stdin: Option<Bytes>,
) -> Result<LazyValue, FunctionError> {
    /// Wrap an IO error
    fn io_error(
        program: &str,
        arguments: &[String],
        error: io::Error,
    ) -> RenderError {
        RenderError::from(FunctionError::CommandInit {
            program: program.to_owned(),
            arguments: arguments.to_owned(),
            error,
        })
    }

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
    let future = async {
        let span = debug_span!("Running command", ?program, ?arguments);

        // Spawn the command process
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
        let handle = tokio::spawn(
            async move {
                let result = child.wait().await;
                debug!(?result, "Command finished");
                result
            }
            .instrument(span),
        );

        // After stdout is done, we'll check the status code of the process to
        // make sure it succeeded. This gets chained on to the end of
        // the stream
        let status_future = async move {
            let status = handle
                .await
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
        };
        Ok(reader_stream(stdout).chain(status_future.into_stream()))
    };

    let stream = future.try_flatten_stream().boxed();

    Ok(LazyValue::Stream {
        source: StreamSource::Command { command },
        stream,
    })
}

/// Concatenate any number of strings together
///
/// **Parameters**
///
/// - `elements`: Strings to concatenate together. Any non-string values will be
///   stringified
///
/// **Examples**
///
/// ```sh
/// {{ concat(['My name is ', name, ' and I am ', age]) }} => "My name is Ted and I am 37"
/// {{ file("data.json") | jsonpath("$.users[*].name") | concat() }} => Concatenate all names in the JSON together
/// ```
#[template]
pub fn concat(elements: Vec<String>) -> String {
    elements.into_iter().join("")
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
#[template]
pub fn debug(value: Value) -> Value {
    println!("{value:?}");
    value
}

/// Get the value of an environment variable, or `""` if not set.
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
#[template]
pub fn env(variable: String) -> String {
    env::var(variable).unwrap_or_default()
}

/// Load contents of a file. While the output type is `bytes`,
/// [in most cases you can use it interchangeably as a
/// string](../user_guide/templates/values.md#bytes-vs-string). `bytes`
/// support means you can also use this to load binary files such as images,
/// which can be useful for request bodies.
///
/// This function supports [streaming](../user_guide/streaming.html).
///
/// **Parameters**
///
/// - `path`: Path to the file to read, relative to the collection file
///   (`slumber.yml`) in use. A leading `~` will be expanded to your home
///   directory (`$HOME`).
///
/// **Errors**
///
/// - If an I/O error occurs while opening the file (e.g. file missing)
///
/// **Examples**
///
/// ```sh
/// {{ file("config.json") }} => Contents of config.json file
/// ```
#[template]
pub fn file(
    #[context] context: &SingleRenderContext<'_>,
    path: String,
) -> LazyValue {
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
    LazyValue::Stream {
        source,
        stream: future.try_flatten_stream().boxed(),
    }
}

/// Convert a value to a float
///
/// **Parameters**
///
/// - `value`: Value to convert
///
/// **Errors**
///
/// - If `value` is a string or byte string that doesn't parse to a float, or an
///   inconvertible type such as an array
///
/// **Examples**
///
/// ```sh
/// {{ float('3.5') }} => 3.5
/// {{ float(b'3.5') }} => 3.5
/// {{ float(3) }} => 3.0
/// {{ float(null) }} => 0.0
/// {{ float(false) }} => 0.0
/// {{ float(true) }} => 1.0
/// ```
#[template]
pub fn float(value: Value) -> Result<f64, ValueError> {
    match value {
        Value::Null => Ok(0.0),
        Value::Boolean(b) => Ok((b).into()),
        Value::Float(f) => Ok(f),
        Value::Integer(i) => Ok(i as f64),
        Value::String(s) => Ok(s.parse()?),
        Value::Bytes(bytes) => Ok(std::str::from_utf8(&bytes)?.parse()?),
        Value::Array(_) | Value::Object(_) => Err(ValueError::Type {
            expected: Expected::OneOf(&[
                &Expected::Float,
                &Expected::Integer,
                &Expected::Boolean,
                &Expected::Custom("string/bytes that parse to a float"),
            ]),
        }),
    }
}

/// Convert a value to an int
///
/// **Parameters**
///
/// - `value`: Value to convert
///
/// **Errors**
///
/// - If `value` is a string or byte string that doesn't parse to an integer, or
///   an inconvertible type such as an array
///
/// **Examples**
///
/// ```sh
/// {{ integer('3') }} => 3
/// {{ integer(b'3') }} => 3
/// {{ integer(3.5) }} => 3
/// {{ integer(null) }} => 0
/// {{ integer(false) }} => 0
/// {{ integer(true) }} => 1
/// ```
#[template]
pub fn integer(value: Value) -> Result<i64, ValueError> {
    match value {
        Value::Null => Ok(0),
        Value::Boolean(b) => Ok(b.into()),
        Value::Float(f) => Ok(f as i64),
        Value::Integer(i) => Ok(i),
        Value::String(s) => Ok(s.parse()?),
        Value::Bytes(bytes) => Ok(std::str::from_utf8(&bytes)?.parse()?),
        Value::Array(_) | Value::Object(_) => Err(ValueError::Type {
            expected: Expected::OneOf(&[
                &Expected::Integer,
                &Expected::Float,
                &Expected::Boolean,
                &Expected::Custom("string/bytes that parse to an integer"),
            ]),
        }),
    }
}

/// Transform a JSON value using a [jq](https://jqlang.org/manual/) query. For
/// simple queries, [`jsonpath`](#jsonpath) is often simpler to use, but `jq` is
/// much more powerful and flexible. In particular, `jq` can be used to
/// construct new JSON values, while JSONPath can only extract from existing
/// values.
///
/// This function is most useful when used after a data-providing function such
/// as [`file`](#file) or [`response`](#response).
///
/// This relies on a pure Rust reimplmentation of `jq` called
/// [jaq](https://github.com/01mf02/jaq). While largely compliant with the
/// original `jq` behavior, [there are some differences](https://github.com/01mf02/jaq?tab=readme-ov-file#differences-between-jq-and-jaq).
///
/// **Parameters**
///
/// - `value`: JSON value to query. If this is a string or bytes, it will be
///   parsed as JSON before being queried. If it's already a structured value
///   (bool, array, etc.), it will be mapped directly to JSON. This value is
///   typically piped in from the output of `response()` or `file()`.
/// - `query`: `jq` query string (see `jq` docs linked above)
/// - `mode` (default: `"auto"`): How to handle multiple results (see table
///   below)
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
/// **Errors**
///
/// - If `value` is a string with invalid JSON
/// - If the query returns no results and `mode='auto'` or `mode='single'`
/// - If the query returns 2+ results and `mode='single'`
///
/// **Examples**
///
/// ```sh
/// {{ response('get_user') | jq(".first_name") }} => "Alice"
/// ```
#[template]
pub fn jq(
    query: JaqQuery,
    value: JsonValue, // Value last so it can be piped in
    #[kwarg] mode: JsonQueryMode,
) -> Result<Value, FunctionError> {
    // This uses jaq instead of jq-rs because the build process for jq-rs is
    // not straightforward. I don't want to add any unnecessary build deps

    // jaq has some very generic names, so use a local import to prevent
    // cluttering the entire module
    use jaq_core::{Ctx, RcIter};

    // iterator over the output values
    let inputs = RcIter::new(core::iter::empty());
    let results = query
        .filter
        .run((Ctx::new([], &inputs), jaq_json::Val::from(value.0)));

    let items = results
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| FunctionError::Jq(error.to_string()))?;
    mode.get_values(query.query, items.into_iter().map(serde_json::Value::from))
}

/// Precompiled jaq query
struct JaqQuery {
    /// Original query
    query: String,
    /// Compiled jaq filter
    filter: jaq_core::Filter<jaq_core::Native<jaq_json::Val>>,
}

impl FromStr for JaqQuery {
    type Err = ValueError;

    fn from_str(query: &str) -> Result<Self, ValueError> {
        // jaq has some very generic names, so use a local import to prevent
        // cluttering the entire module
        use jaq_core::{
            Compiler,
            load::{Arena, File, Loader},
        };

        /// The error formatting is pretty junk because jaq's error types are
        /// terrible. I did my best :)
        fn format_errors<E>(errors: Vec<E>, f: impl Fn(E) -> String) -> String {
            errors
                .into_iter()
                // The first term is the file+code, which we can throw away. We
                // know the code didn't come from a file, and the code will be
                // attached via WithValue already.
                .map(f)
                .format("; ")
                .to_string()
        }

        let program = File {
            code: query,
            path: (),
        };

        // We could potentially put these in statics if the performance is bad
        let loader = Loader::new(jaq_std::defs());
        let arena = Arena::default();

        // Parse the filter
        let modules = loader.load(&arena, program).map_err(|errors| {
            ValueError::other(format_errors(errors, |(_, error)| match error {
                jaq_core::load::Error::Io(items) => {
                    format_errors(items, |(path, error)| {
                        format!("error loading `{path}`: {error}")
                    })
                }
                jaq_core::load::Error::Lex(items) => {
                    format_errors(items, |(expected, actual)| {
                        format!(
                            "expected {expected}, got `{actual}`",
                            expected = expected.as_str()
                        )
                    })
                }
                jaq_core::load::Error::Parse(items) => {
                    format_errors(items, |(expected, actual)| {
                        format!(
                            "expected {expected}, got `{actual}`",
                            expected = expected.as_str()
                        )
                    })
                }
            }))
        })?;

        // Compile the filter
        let filter = Compiler::default()
            .with_funs(jaq_std::funs())
            .compile(modules)
            .map_err(|errors| {
                // Yes, there's TWO levels of error lists!!
                ValueError::other(format_errors(errors, |(_, errors)| {
                    format_errors(errors, |(function, _)| {
                        // This is seemingly the only possible compile error
                        format!("Undefined function `{function}`")
                    })
                }))
            })?;

        Ok(Self {
            query: query.to_owned(),
            filter,
        })
    }
}

impl_try_from_value_str!(JaqQuery);

/// Parse a JSON string to a template value.
///
/// **Parameters**
///
/// - `value`: JSON string
///
/// **Errors**
///
/// - If `value` is not valid JSON
///
/// **Examples**
///
/// ```sh
/// {{ json_parse('{"name": "Alice"}') }} => {"name": "Alice"}
/// # Commonly combined with file() or response() because they spit out raw JSON
/// {{ file('body.json') | json_parse() }} => {"name": "Alice"}"
/// {{ response('get_user') | json_parse() }} => {"name": "Alice"}"
/// ```
///
/// This can be used in `json` request bodies to create dynamic non-string
/// values.
///
/// ```yaml
/// body:
///   type: json
///   data: {
///     "data": "{{ response('get_user') | json_parse() }}"
///   }
/// ```
///
/// This will render a request body like:
///
/// ```json
/// {"data": {"name": "Alice"}}
/// ```
#[template]
pub fn json_parse(value: String) -> Result<serde_json::Value, FunctionError> {
    serde_json::from_str(&value).map_err(FunctionError::JsonParse)
}

/// Transform a JSON value using a JSONPath query. See
/// [JSONPath specification](https://datatracker.ietf.org/doc/html/rfc9535) or
/// [jsonpath.com](https://jsonpath.com/) for query syntax. For more complex
/// queries, you may want to use [`jq`](#jq) instead.
///
/// This function is most useful when used after a data-providing function such
/// as [`file`](#file) or [`response`](#response).
///
/// **Parameters**
///
/// - `value`: JSON value to query. If this is a string or bytes, it will be
///   parsed as JSON before being queried. If it's already a structured value
///   (bool, array, etc.), it will be mapped directly to JSON. This value is
///   typically piped in from the output of `response()` or `file()`.
/// - `query`: JSONPath query string
/// - `mode` (default: `"auto"`): How to handle multiple results (see table
///   below)
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
/// **Errors**
///
/// - If `value` is a string with invalid JSON
/// - If the query returns no results and `mode='auto'` or `mode='single'`
/// - If the query returns 2+ results and `mode='single'`
///
/// **Examples**
///
/// ```sh
/// {{ response('get_user') | jsonpath("$.first_name") }} => "Alice"
/// ```
#[template]
pub fn jsonpath(
    query: JsonPath,
    value: JsonValue, // Value last so it can be piped in
    #[kwarg] mode: JsonQueryMode,
) -> Result<Value, FunctionError> {
    let query = query.0;
    let node_list = query.query(&value.0);

    // Convert the node list to a template value based on mode
    mode.get_values(query.to_string(), node_list.into_iter().cloned())
}

/// Wrapper for [serde_json_path::JsonPath] to enable implementing
/// [TryFromValue]
#[derive(Debug, FromStr)]
pub struct JsonPath(serde_json_path::JsonPath);

impl_try_from_value_str!(JsonPath);

/// Wrapper for a JSON value to customize decoding. Strings are parsed as JSON
/// instead of being treated as a JSON string literal. You can't really do
/// anything with a JSONPath or jq on a string so when a user pipes a string (or
/// bytes) in, it's probably the output of a response or file that needs to
/// be parsed. By parsing here, we save them an intermediate call to
/// `json_parse()`.
pub struct JsonValue(serde_json::Value);

impl TryFromValue for JsonValue {
    fn try_from_value(value: Value) -> Result<Self, WithValue<ValueError>> {
        let json_value = match value {
            // Strings and bytes are treated as encoded JSON and parsed.
            // See struct doc for explanation
            Value::String(s) => serde_json::from_str(&s)
                .map_err(|error| WithValue::new(s.into(), error))?,
            Value::Bytes(b) => serde_json::from_slice(&b)
                .map_err(|error| WithValue::new(b.into(), error))?,
            // Everything else is mapped literally
            Value::Null => serde_json::Value::Null,
            Value::Boolean(b) => b.into(),
            Value::Integer(i) => i.into(),
            Value::Float(f) => f.into(),
            // Strings nested within an object/array will *not* be parsed
            Value::Array(array) => array
                .into_iter()
                .map(serde_json::Value::try_from_value)
                .collect::<Result<_, _>>()?,
            Value::Object(map) => map
                .into_iter()
                .map(|(k, v)| Ok((k, serde_json::Value::try_from_value(v)?)))
                .collect::<Result<_, _>>()?,
        };
        Ok(Self(json_value))
    }
}

/// Control how a jq/JSONPath selector returns 0 vs 1 vs 2+ results
#[derive(Copy, Clone, Debug, Default)]
#[cfg_attr(any(test, feature = "test"), derive(PartialEq))]
pub enum JsonQueryMode {
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

impl JsonQueryMode {
    /// Extract values from a query result according to this mode. This takes an
    /// iterator of JSON values so it works for both jq and jsonpath
    fn get_values<Iter>(
        self,
        query: String,
        values: Iter,
    ) -> Result<Value, FunctionError>
    where
        Iter: IntoIterator<Item = serde_json::Value>,
    {
        enum Case {
            None,
            One,
            Many,
        }

        // For each mode, there are 3 cases to handle: 0, 1, and 2+ values.
        // We'll peek at the first two values to see which case we're handling
        let mut iter = itertools::peek_nth(values);
        let case =
            match (iter.peek_nth(0).is_some(), iter.peek_nth(1).is_some()) {
                (false, false) => Case::None,
                (true, false) => Case::One,
                (true, true) => Case::Many,
                (false, true) => unreachable!(),
            };

        // Handle each possible case pair
        match (self, case) {
            (Self::Auto | Self::Single, Case::None) => {
                Err(FunctionError::JsonQueryNoResults { query })
            }
            (Self::Auto, Case::One) => {
                Ok(Value::from_json(iter.next().unwrap()))
            }
            (Self::Auto, Case::Many) => {
                Ok(Value::Array(iter.map(Value::from_json).collect()))
            }

            (Self::Single, Case::One) => {
                Ok(Value::from_json(iter.next().unwrap()))
            }
            (Self::Single, Case::Many) => {
                Err(FunctionError::JsonQueryTooMany {
                    query,
                    actual_count: iter.count(),
                })
            }

            // Case doesn't matter for mode=array, because we always return an
            // array
            (Self::Array, _) => {
                Ok(Value::Array(iter.map(Value::from_json).collect()))
            }
        }
    }
}

// Manual implementation provides the best error messages
impl FromStr for JsonQueryMode {
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

impl_try_from_value_str!(JsonQueryMode);

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
/// **Errors**
///
/// - If the user doesn't give a response
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
#[template]
pub async fn prompt(
    #[context] context: &SingleRenderContext<'_>,
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
    // static output string from the preview prompter, i.e. instead of showing
    // "<prompt>", we show "••••••••". It's "technically" right and plays well
    // in tests. Also it reminds users that a prompt is sensitive in the TUI
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
/// - `recipe_id`: ID (**not** name) of the recipe to load the response from
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
/// **Errors**
///
/// - If `recipe` isn't in the collection
/// - If there is no request in history and `trigger='never'`
/// - If a request is triggered and failed
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
#[template]
pub async fn response(
    #[context] context: &SingleRenderContext<'_>,
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
/// - `recipe_id`: ID (**not** name) of the recipe to load the response from
/// - `header`: Name of the header to extract (case-insensitive)
/// - `trigger`: When to execute the upstream request vs using the cached
///   response; [see `response`](#response)
///
/// **Errors**
///
/// - If `recipe` isn't in the collection
/// - If there is no request in history and `trigger='never'`
/// - If a request is triggered and failed
///
/// **Examples**
///
/// ```sh
/// # Fetch current rate limit, refreshed if older than 5 minutes
/// {{ response_header("get_rate_limit", "X-Rate-Limit", trigger="5m") }} => Value of X-Rate-Limit response header
/// ```
#[template]
pub async fn response_header(
    #[context] context: &SingleRenderContext<'_>,
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
/// - `options`: List of options to choose from. Each option can be either a
///   string *or* an object with the fields `"label"` and `"value"`. If an
///   object is given, the `"label"` field will be shown to the user, but the
///   corresponding `"value"` field will be returned.
/// - `message`: Descriptive message to display to the user
///
/// **Errors**
///
/// - If `options` is empty
/// - If the user doesn't give a response
///
/// **Examples**
///
/// ```sh
/// {{ select(["dev", "staging", "prod"]) }} => "dev"
/// # Custom prompt message
/// {{ select(["GET", "POST", "PUT"], message="HTTP method") }} => "POST"
/// # "label" will be shown to the user, but the corresponding "value" will be returned
/// {{ select([{"label": "Sam", "value": 1}, {"label": "Mike", "value": 2}]) }} => 2
/// # jq() can be used to construct labelled options dynamically
/// {{ [{"name": "Sam", "id": 1}, {"name": "Mike", "id": 2}]
///     | jq('[.[] | {label: .name, value: .id}]')
///     | select(message="Select user") }}
/// ```
#[template]
pub async fn select(
    #[context] context: &SingleRenderContext<'_>,
    options: Vec<SelectOption>,
    #[kwarg] message: Option<String>,
) -> Result<Value, FunctionError> {
    // If there are no options, we can't show anything meaningful to the user.
    // We *could* just return an empty string but that may be confusing.
    // Something probably went wrong upstream so return an error.
    if options.is_empty() {
        return Err(FunctionError::SelectNoOptions);
    }

    let (tx, rx) = oneshot::channel();
    context.prompter.select(Select {
        message: message.unwrap_or_default(),
        options,
        channel: tx.into(),
    });
    rx.await.map_err(|_| FunctionError::PromptNoReply)
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
#[template]
pub fn sensitive(
    #[context] context: &SingleRenderContext<'_>,
    value: String,
) -> String {
    mask_sensitive(context, value)
}

/// Stringify a value. Any value can be converted to a string except for
/// non-UTF-8 bytes
///
/// **Parameters**
///
/// - `value`: Value to stringify
///
/// **Errors**
///
/// - If `value` is a byte string that isn't valid UTF-8
///
/// **Examples**
///
/// ```sh
/// {{ string('hello') }} => "hello"
/// {{ string(b'hello') }} => "hello"
/// {{ string([1, 2, 3]) }} => "[1, 2, 3]"
/// ```
#[template]
pub fn string(value: Value) -> Result<String, ValueError> {
    String::try_from_value(value).map_err(WithValue::into_error)
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
#[template]
pub fn trim(value: String, #[kwarg] mode: TrimMode) -> String {
    match mode {
        TrimMode::Start => value.trim_start().to_string(),
        TrimMode::End => value.trim_end().to_string(),
        TrimMode::Both => value.trim().to_string(),
    }
}

fn mask_sensitive(context: &SingleRenderContext<'_>, value: String) -> String {
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
