//! Functions available in templates

use crate::{
    collection::RecipeId,
    render::{FunctionError, Prompt, Select, TemplateContext},
};
use bytes::Bytes;
use serde::{
    Deserialize,
    de::{self, Visitor, value::SeqDeserializer},
};
use serde_json_path::{JsonPath, NodeList};
use slumber_macros::template;
use slumber_template::TryFromValue;
use slumber_util::TimeSpan;
use std::{env, fmt::Debug, process::Stdio, sync::Arc};
use tokio::{fs, io::AsyncWriteExt, process::Command, sync::oneshot};
use tracing::{debug, debug_span};

/// Run a command in a subprocess
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

/// Print a value to stdout, returning the same value
#[template(TemplateContext)]
pub fn debug(value: slumber_template::Value) -> slumber_template::Value {
    println!("{value:?}");
    value
}

/// Get the value of an environment variable. Return `None` if the variable is
/// not set
#[template(TemplateContext)]
pub fn env(variable: String) -> Option<String> {
    env::var(variable).ok()
}

/// Load contents of a file
#[template(TemplateContext)]
pub async fn file(path: String) -> Result<Bytes, FunctionError> {
    let bytes = fs::read(&path).await.map_err(|error| FunctionError::File {
        path: path.into(),
        error,
    })?;
    Ok(bytes.into())
}

/// Control how a JSONPath selector returns 0 vs 1 vs 2+ results
#[derive(Copy, Clone, Debug, Default, Deserialize)]
#[cfg_attr(any(test, feature = "test"), derive(PartialEq))]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
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

/// Transform a JSON value using a JSONPath query
#[template(TemplateContext)]
pub fn jsonpath(
    // Value first so it can be piped in
    value: serde_json::Value,
    #[serde] query: JsonPath,
    #[kwarg]
    #[serde]
    mode: JsonPathMode,
) -> Result<slumber_template::Value, FunctionError> {
    fn node_list_to_value(node_list: NodeList) -> slumber_template::Value {
        slumber_template::Value::deserialize(SeqDeserializer::new(
            node_list.into_iter(),
        ))
        // This conversion is infallible because JSON is a subset of Value and
        // the NodeList produces an array of JSON values
        .unwrap()
    }

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

/// Prompt the user to enter a text value
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

/// Load the most recent response body for a recipe and the current profile
#[template(TemplateContext)]
pub async fn response(
    #[context] context: &TemplateContext,
    recipe_id: RecipeId,
    #[kwarg]
    #[serde]
    trigger: RequestTrigger,
) -> Result<Bytes, FunctionError> {
    let response = context.get_latest_response(&recipe_id, trigger).await?;
    let body = match Arc::try_unwrap(response) {
        Ok(response) => response.body,
        Err(response) => response.body.clone(),
    };
    Ok(body.into_bytes())
}

/// Load a header value from the most recent response for a recipe and the
/// current profile
#[template(TemplateContext)]
pub async fn response_header(
    #[context] context: &TemplateContext,
    recipe_id: RecipeId,
    header: String,
    #[kwarg]
    #[serde]
    trigger: RequestTrigger,
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

/// Ask the user to select a value from a list
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

/// Hide a sensitive value if the context has show_sensitive disabled
#[template(TemplateContext)]
pub fn sensitive(
    #[context] context: &TemplateContext,
    value: String,
) -> String {
    mask_sensitive(context, value)
}

/// Trim whitespace from a string
#[template(TemplateContext)]
pub fn trim(
    value: String,
    #[kwarg]
    #[serde]
    mode: TrimMode,
) -> String {
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
                formatter.write_str(
                    "\"never\", \"noHistory\", \"always\", or a duration \
                    string such as \"1h\"",
                )
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                match v {
                    // If you add a case here, update the expecting string too
                    "never" => Ok(RequestTrigger::Never),
                    "noHistory" => Ok(RequestTrigger::NoHistory),
                    "always" => Ok(RequestTrigger::Always),
                    // Anything else is parsed as a duration
                    _ => {
                        let duration =
                            v.parse::<TimeSpan>().map_err(de::Error::custom)?;
                        Ok(RequestTrigger::Expire { duration })
                    }
                }
            }
        }

        deserializer.deserialize_any(RequestTriggerVisitor)
    }
}

/// Trim whitespace from a string
#[derive(Copy, Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum TrimMode {
    /// Trim the start of the output
    Start,
    /// Trim the end of the output
    End,
    /// Trim the start and end of the output
    #[default]
    Both,
}

impl TryFromValue for RecipeId {
    fn try_from_value(
        value: slumber_template::Value,
    ) -> Result<Self, slumber_template::RenderError> {
        String::try_from_value(value).map(RecipeId::from)
    }
}
