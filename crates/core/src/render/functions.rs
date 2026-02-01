//! Functions available in templates

use crate::{
    collection::RecipeId,
    render::{FunctionError, Prompt, SelectOption, SingleRenderContext},
};
use base64::{Engine, prelude::BASE64_STANDARD};
use bytes::Bytes;
use derive_more::FromStr;
use futures::{FutureExt, Stream, StreamExt, TryFutureExt, TryStreamExt};
use itertools::Itertools;
use regex::Regex;
use serde::{Deserialize, de::IntoDeserializer};
use slumber_macros::template;
use slumber_template::{
    Expected, LazyValue, RenderError, StreamSource, TryFromValue, Value,
    ValueError, WithValue, impl_try_from_value_str,
};
use slumber_util::{TimeSpan, paths::expand_home};
use std::{
    env, fmt::Debug, io, path::PathBuf, process::Stdio, str::FromStr, sync::Arc,
};
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
///   - input: base64("dGVzdA==", decode=true)
///     output: '"test"'
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

/// ```notrust
/// description: Convert a value to a boolean. Empty values such as `0`, `""` or `[]`
///   convert to `false`. Anything else converts to `true`.
/// parameters:
///   value:
///     description: Value to convert
/// return: Boolean representation of the input
/// examples:
///   - input: boolean(null)
///     output: false
///   - input: boolean(0)
///     output: false
///   - input: boolean(1)
///     output: true
///   - input: boolean('')
///     output: false
///   - input: boolean('0')
///     output: true
///   - input: boolean([])
///     output: false
///   - input: boolean([0])
///     output: true
/// ```
#[template]
pub fn boolean(value: Value) -> bool {
    value.to_bool()
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
/// return: Stdout output as bytes. May be returned as a stream (LazyValue).
/// errors:
///   - If the command fails to initialize (e.g. program unknown)
///   - If the subprocess exits with a non-zero status code
/// examples:
///   - input: command(["echo", "hello"])
///     output: "hello\n"
///   - input: command(["grep","1"], stdin="line 1\nline2")
///     output: "line 1\n"
/// ```
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

    Ok(LazyValue::Stream {
        source: StreamSource::Command { command },
        stream,
    })
}

/// ```notrust
/// description: Concatenate any number of strings together
/// tags: [array, string]
/// parameters:
///   elements:
///     description: Strings to concatenate together. Any non-string values will be stringified
/// return: Concatenated string
/// examples:
///   - input: concat(['My name is ', name, ' and I am ', age])
///     output: "My name is Ted and I am 37"
///   - input: file("data.json") | jsonpath("$.users[*].name") | concat()
///     output: "TedSteveSarah"
/// ```
#[template]
pub fn concat(elements: Vec<String>) -> String {
    elements.into_iter().join("")
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
#[template]
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
#[template]
pub fn env(variable: String, #[kwarg] default: String) -> String {
    env::var(variable).unwrap_or(default)
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

/// ```notrust
/// description: Convert a value to a float
/// tags: [number]
/// parameters:
///   value:
///     description: Value to convert
/// return: Floating point representation (f64)
/// errors:
///   - If `value` is a string or byte string that doesn't parse to a float
///   - If `value` is an inconvertible type such as an array or object
/// examples:
///   - input: float('3.5')
///     output: 3.5
///   - input: float(b'3.5')
///     output: 3.5
///   - input: float(3)
///     output: 3.0
///   - input: float(null)
///     output: 0.0
///   - input: float(false)
///     output: 0.0
///   - input: float(true)
///     output: 1.0
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

/// ```notrust
/// description: >-
///   Get one element from a string, bytes, or array
///
///   For strings, the index is in terms of *characters*, not bytes.
/// tags: [string, array]
/// parameters:
///   index:
///     description: Index of the element to return, starting at 0.
///       Negative values count backwards from the end.
///   sequence:
///     description: String, bytes, or array to index into
/// return: Value at `index`. If `index >= length`, return `null`
/// examples:
///   - input: "[0, 1, 2] | index(1)"
///     output: "1"
///   - input: "'abc' | index(1)"
///     output: "'b'"
///   - input: "'abc' | index(-1)"
///     output: "'c'"
///     comment: Negative indexes count back from the end
///   - input: "'abc' | index(3)"
///     output: "null"
///   - input: "'nägemist' | index(1)"
///     output: "'ä'"
///     comment: String indexes are in terms of characters. Multi-byte UTF-8
///       characters count as a single element
///   - input: "b'nägemist' | index(1)"
///     output: "b'\xc3'"
///     comment: Bytes indexes are in terms of bytes, not UTF-8 characters
/// ```
#[template]
pub fn index(index: i64, sequence: Sequence) -> Option<Value> {
    let index = sequence.wrap_index(index);
    if index >= sequence.len() as usize {
        return None;
    }

    let value = match sequence {
        Sequence::String(string) => string
            .chars()
            .nth(index)
            .unwrap()
            // Safety: we checked index against len() above, and that length is
            // based on the char length
            .to_string()
            .into(),
        Sequence::Bytes(bytes) => bytes.slice(index..=index).into(),
        Sequence::Array(mut array) => array.swap_remove(index),
    };
    Some(value)
}

/// ```notrust
/// description: Convert a value to an int
/// tags: [number]
/// parameters:
///   value:
///     description: Value to convert
/// return: Integer representation (i64)
/// errors:
///   - If `value` is a string or byte string that doesn't parse to an integer
///   - If `value` is an inconvertible type such as an array or object
/// examples:
///   - input: integer('3')
///     output: 3
///   - input: integer(b'3')
///     output: 3
///   - input: integer(3.5)
///     output: 3
///   - input: integer(null)
///     output: 0
///   - input: integer(false)
///     output: 0
///   - input: integer(true)
///     output: 1
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

/// ```notrust
/// description: >-
///   Join a list of strings with a separator
///
///   `join` is the inverse of [`split`](#split). `join(sep, split(sep, value))`
///   always yields `value`.
/// tags: [string, array]
/// parameters:
///   separator:
///     description: String to join with
///   values:
///     description: Array to join
/// return: Joined string
/// examples:
///   - input: "['a', 'b', 'c'] | join(',')"
///     output: "'a,b,c'"
///   - input: "[1, 2, 3] | join(',')"
///     output: "'1,2,3'"
///     comment: Non-string values are coerced to strings
/// ```
#[template]
pub fn join(separator: String, values: Vec<String>) -> String {
    values.join(&separator)
}

/// ```notrust
/// description: Transform a JSON value using a `jq` query. Uses the `jaq` Rust implementation.
/// tags: [json]
/// parameters:
///   query:
///     description: "`jq` query string"
///   value:
///     description: JSON value to query. Strings/bytes will be parsed as JSON first.
///   mode:
///     description: How to handle multiple results (auto/single/array)
///     default: "auto"
/// return: Resulting template `Value`
/// errors:
///   - If `value` is a string with invalid JSON
///   - If the query returns no results and `mode='auto'` or `mode='single'`
///   - If the query returns 2+ results and `mode='single'`
/// examples:
///   - input: response('get_user') | jq(".first_name")
///     output: "Alice"
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

/// ```notrust
/// description: Parse a JSON string to a template value
/// tags: [json]
/// parameters:
///   value:
///     description: JSON string
/// return: Parsed JSON as `serde_json::Value`
/// errors:
///   - If `value` is not valid JSON
/// examples:
///   - input: "json_parse('{\"name\": \"Alice\"}')"
///     output: '{"name": "Alice"}'
///   - input: "file('body.json') | json_parse()"
///     output: '{"name": "Alice"}'
/// ```
#[template]
pub fn json_parse(value: String) -> Result<serde_json::Value, FunctionError> {
    serde_json::from_str(&value).map_err(FunctionError::JsonParse)
}

/// ```notrust
/// description: Transform a JSON value using a JSONPath query
/// tags: [json]
/// parameters:
///   value:
///     description: JSON value to query. Strings/bytes will be parsed as JSON before querying.
///   query:
///     description: JSONPath query string
///   mode:
///     description: How to handle multiple results (auto/single/array)
///     default: "auto"
/// return: Resulting template `Value`
/// errors:
///   - If `value` is a string with invalid JSON
///   - If the query returns no results and `mode='auto'` or `mode='single'`
///   - If the query returns 2+ results and `mode='single'`
/// examples:
///   - input: response('get_user') | jsonpath("$.first_name")
///     output: "Alice"
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

/// ```notrust
/// description: Control how a jq/JSONPath selector returns 0 vs 1 vs 2+ results
/// parameters: {}
/// return: Enum describing mode (auto/single/array)
/// examples: []
/// ```
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

/// ```notrust
/// description: Convert a string to lowercase
/// tags: [string]
/// parameters:
///   value:
///     description: String to convert
/// return: Lowercased string
/// examples:
///   - input: lower("HELLO")
///     output: "hello"
///   - input: lower("NÄGEMIST")
///     output: "nägemist"
///     comment: UTF-8 characters are converted as well
/// ```
#[template]
pub fn lower(value: String) -> String {
    value.to_lowercase()
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
#[template]
pub async fn prompt(
    #[context] context: &SingleRenderContext<'_>,
    #[kwarg] message: Option<String>,
    #[kwarg] default: Option<String>,
    #[kwarg] sensitive: bool,
) -> Result<String, FunctionError> {
    let (tx, rx) = oneshot::channel();
    context.prompter.prompt(Prompt::Text {
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

/// ```notrust
/// description: Replace all occurrences of `from` in `value` with `to`
/// tags: [string]
/// parameters:
///   from:
///     description: Pattern to be replaced
///   to:
///     description: String to replace each occurrence of `from` with
///   value:
///     description: String to split
///   regex:
///     description: If `true`, `from` will be parsed as a
///       [regular expression](https://en.wikipedia.org/wiki/Regular_expression)
///       instead of a plain string.
///     default: false
///   n:
///     description: Maximum number of replacements to make, starting from the
///       start of the string. If `null`, make all possible replacements
///     default: "null"
/// return: Array of separated string segments
/// errors:
///   - If `regex=true` but `from` is not a valid regex
/// examples:
///   - input: "'banana' | replace('na', 'ma')"
///     output: "'bamama'"
///   - input: "'banana' | replace('[ab]', 'x', regex=true)"
///     output: "'xxnxnx'"
///     comment: Replace a or b with x
///   - input: "'banana' | replace('na', 'ma', n=1)"
///     output: "'bamana'"
///   - input: "'bananan' | replace('nan', 'mam')"
///     output: "'bamaman'"
///     comment: Overlapping instances of `to` are NOT all replaced
/// ```
#[template]
pub fn replace(
    from: String,
    to: String,
    value: String,
    #[kwarg] regex: bool,
    #[kwarg] n: Option<u32>,
) -> Result<String, FunctionError> {
    if regex {
        let regex = Regex::new(&from)?;
        if let Some(n) = n {
            Ok(regex.replacen(&value, n as usize, to).into_owned())
        } else {
            Ok(regex.replace_all(&value, to).into_owned())
        }
    } else {
        // Plain string replace
        if let Some(n) = n {
            Ok(value.replacen(&from, &to, n as usize))
        } else {
            Ok(value.replace(&from, &to))
        }
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
    context.prompter.prompt(Prompt::Select {
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
#[template]
pub fn sensitive(
    #[context] context: &SingleRenderContext<'_>,
    value: String,
) -> String {
    mask_sensitive(context, value)
}

/// ```notrust
/// description: >-
///   Extract a portion of a string, bytes, or array
///
///   Indexes are zero-based and [inclusive, exclusive)
/// tags: [string, array]
/// parameters:
///   start:
///     description: Index of the first element to include, starting at 0.
///       Negative values count backward from the end.
///   stop:
///     description: Index *after* the last element to include, starting at 0.
///       `null` will slice to the end. Negative values count backwards from the
///       end.
///   sequence:
///     description: String, bytes, or array to slice
/// return: Subslice of the input string/array. If `stop < start`, return an
///   empty slice. If either index outside the range `[0, length]`, it will be
///   clamped to that range.
/// examples:
///   - input: "[0, 1, 2] | slice(1, 2)"
///     output: "[1]"
///   - input: "[0, 1, 2] | slice(1, 3)"
///     output: "[1, 2]"
///   - input: "'abc' | slice(0, 2)"
///     output: "'ab'"
///   - input: "'abc' | slice(0, 0)"
///     output: "''"
///   - input: "'abc' | slice(1, null)"
///     output: "'bc'"
///     comment: Use `null` for `stop` to slice to the end
///   - input: "'abc' | slice(1, -1)"
///     output: "'b'"
///     comment: Negative indexes count back from the end
///   - input: "'abc' | slice(-2, null)"
///     output: "'bc'"
///     comment: Combine the two to get the last n elements
///   - input: "'nägemist' | slice(1, 3)"
///     output: "'äg'"
///     comment: Indexes are in terms of characters. Multi-byte UTF-8 characters
///       count as a single element
///   - input: "b'nägemist' | slice(1, 3)"
///     output: "b'\xc3\xa4'"
///     comment: Bytes indexes are in terms of bytes, not UTF-8 characters
/// ```
#[template]
pub fn slice(start: i64, stop: Option<i64>, sequence: Sequence) -> Sequence {
    let len = sequence.len();
    // null => end of list
    let stop = stop.unwrap_or(len);

    // Clamp values to be no greated than len, then wrap negative indexes to be
    // from the end. We don't want to grab values >len.
    let start = sequence.wrap_index(start.min(len));
    let stop = sequence.wrap_index(stop.min(len));

    // Special case - return empty
    if stop < start {
        return match sequence {
            Sequence::String(_) => Sequence::String(String::new()),
            Sequence::Bytes(_) => Sequence::Bytes(Bytes::new()),
            Sequence::Array(_) => Sequence::Array(vec![]),
        };
    }

    match sequence {
        Sequence::String(string) => {
            let string = string
                .chars()
                .skip(start)
                .take(stop - start)
                .collect::<String>();
            Sequence::String(string)
        }
        Sequence::Bytes(bytes) => Sequence::Bytes(bytes.slice(start..stop)),
        Sequence::Array(mut array) => {
            let array = array.drain(start..stop).collect::<Vec<_>>();
            Sequence::Array(array)
        }
    }
}

/// ```notrust
/// description: Split a string on a separator
/// tags: [string]
/// parameters:
///   separator:
///     description: String to split on
///   value:
///     description: String to split
///   n:
///     description: Maximum number of times to split. If `null`, split as many times as
///       possible
///     default: "null"
/// return: Array of separated string segments
/// examples:
///   - input: "'a,b,c' | split(',')"
///     output: "['a', 'b', 'c']"
///   - input: "'a,b,c' | split(',', n=1)"
///     output: "['a', 'b,c']"
///   - input: "'a,b,c' | split('')"
///     output: "['', 'a', ',', 'b', ',', 'c', '']"
///   - input: "'' | split(',')"
///     output: "['']"
/// ```
#[template]
pub fn split(
    separator: String,
    value: String,
    #[kwarg] n: Option<u32>,
) -> Vec<String> {
    if let Some(n) = n {
        // In splitn, n is the number of elements returned, therefore one more
        // than the number of splits done. I think that's unintuitive
        // though, so we have to do +1 to get the number of terms.
        if n == 0 {
            vec![value]
        } else {
            value
                .splitn((n + 1) as usize, &separator)
                .map(String::from)
                .collect()
        }
    } else {
        value.split(&separator).map(String::from).collect()
    }
}

/// ```notrust
/// description: Stringify a value. Any value can be converted to a string
///   except for non-UTF-8 bytes
/// tags: [string]
/// parameters:
///   value:
///     description: Value to stringify
/// return: String representation
/// errors:
///   - If `value` is a byte string that isn't valid UTF-8
/// examples:
///   - input: string('hello')
///     output: "hello"
///   - input: string(b'hello')
///     output: "hello"
///   - input: string([1, 2, 3])
///     output: "[1, 2, 3]"
/// ```
#[template]
pub fn string(value: Value) -> Result<String, ValueError> {
    String::try_from_value(value).map_err(WithValue::into_error)
}

/// ```notrust
/// description: Trim whitespace from the beginning and/or end of a string
/// tags: [string]
/// parameters:
///   value:
///     description: String to trim (typically piped in)
///   mode:
///     description: Section of the string to trim (start/end/both)
///     default: "both"
/// return: Trimmed string
/// examples:
///   - input: trim("  hello  ")
///     output: "hello"
///   - input: trim("  hello  ", mode="start")
///     output: "hello  "
///   - input: command(["echo", "hello"]) | trim()
///     output: "hello"
/// ```
#[template]
pub fn trim(value: String, #[kwarg] mode: TrimMode) -> String {
    match mode {
        TrimMode::Start => value.trim_start().to_string(),
        TrimMode::End => value.trim_end().to_string(),
        TrimMode::Both => value.trim().to_string(),
    }
}

/// ```notrust
/// description: Convert a string to uppercase
/// tags: [string]
/// parameters:
///   value:
///     description: String to convert
/// return: Uppercased string
/// examples:
///   - input: upper("hello")
///     output: "HELLO"
///   - input: lower("nägemist")
///     output: "NÄGEMIST"
///     comment: UTF-8 characters are converted as well
/// ```
#[template]
pub fn upper(value: String) -> String {
    value.to_uppercase()
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

/// A value that can be indexed and sliced
#[derive(Debug)]
enum Sequence {
    String(String),
    Bytes(Bytes),
    Array(Vec<Value>),
}

impl Sequence {
    /// Get the length of the sequence. Most operations related to this operate
    /// on `i64`s, so the value is converted.
    ///
    /// For strings, the length is the number of *characters*, not bytes.
    fn len(&self) -> i64 {
        (match self {
            Self::String(string) => string.chars().count(),
            Self::Bytes(bytes) => bytes.len(),
            Self::Array(array) => array.len(),
        }) as i64
    }

    /// Coerce an index to be valid for this sequence. Negative values are
    /// wrapped from the end.
    fn wrap_index(&self, index: i64) -> usize {
        let len = self.len();
        if index < 0 && len > 0 {
            // Negative values wrap to the beginning
            index.rem_euclid(len) as usize
        } else {
            index as usize
        }
    }
}

impl TryFromValue for Sequence {
    fn try_from_value(value: Value) -> Result<Self, WithValue<ValueError>> {
        match value {
            Value::String(string) => Ok(Self::String(string)),
            Value::Bytes(bytes) => Ok(Self::Bytes(bytes)),
            Value::Array(array) => Ok(Self::Array(array)),
            _ => Err(WithValue::new(
                value,
                ValueError::Type {
                    expected: Expected::OneOf(&[
                        &Expected::String,
                        &Expected::Bytes,
                        &Expected::Array,
                    ]),
                },
            )),
        }
    }
}

impl From<Sequence> for Value {
    fn from(value: Sequence) -> Self {
        match value {
            Sequence::String(string) => Value::String(string),
            Sequence::Bytes(bytes) => Value::Bytes(bytes),
            Sequence::Array(array) => Value::Array(array),
        }
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
