//! Functions available in templates

use crate::{
    collection::RecipeId,
    render::{FunctionError, Prompt, Select, TemplateContext},
};
use itertools::Itertools;
use serde::{
    Deserialize,
    de::{self, Visitor, value::SeqDeserializer},
};
use serde_json_path::JsonPath;
use slumber_template::{Kwargs, ViaSerde};
use std::{env, fmt::Debug, process::Stdio, sync::Arc, time::Duration};
use tokio::{fs, io::AsyncWriteExt, process::Command, sync::oneshot};
use tracing::{debug, debug_span};

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommandKwargs {
    /// Optional data to pipe to the command via stdin
    #[serde(default)]
    stdin: Option<String>,
}

/// Run a command in a subprocess
pub async fn command(
    (command, Kwargs(CommandKwargs { stdin })): (
        Vec<String>,
        Kwargs<CommandKwargs>,
    ),
) -> Result<Vec<u8>, FunctionError> {
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

    Ok(output.stdout)
}

/// Get the value of an environment variable. Return `None` if the variable is
/// not set
pub fn env((variable,): (String,)) -> Option<String> {
    env::var(variable).ok()
}

/// TODO
pub async fn file((path,): (String,)) -> Result<Vec<u8>, FunctionError> {
    fs::read(&path).await.map_err(|error| FunctionError::File {
        path: path.into(),
        error,
    })
}

/// Transform a JSON value using a JSONPath query
pub fn jsonpath(
    // Value first so it can be piped in
    (ViaSerde(value), ViaSerde(query)): (
        ViaSerde<serde_json::Value>,
        ViaSerde<JsonPath>,
    ),
) -> Result<slumber_template::Value, FunctionError> {
    // TODO support mode?
    let node_list = query.query(&value);
    // Deserialize from the JSON list into a template value. This should be
    // infallible because template values are a superset of JSON
    slumber_template::Value::deserialize(SeqDeserializer::new(
        node_list.into_iter(),
    ))
    .map_err(|_| todo!())
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
pub async fn prompt(
    (
        context,
        Kwargs(PromptKwargs {
            message,
            default,
            sensitive: is_sensitive,
        }),
    ): (&TemplateContext, Kwargs<PromptKwargs>),
) -> Result<String, FunctionError> {
    let (tx, rx) = oneshot::channel();
    context.prompter.prompt(Prompt {
        message: message.unwrap_or_default(),
        default,
        sensitive: is_sensitive,
        channel: tx.into(),
    });
    let output = rx.await.map_err(|_| FunctionError::PromptNoReply)?;

    // If the input was sensitive, we should mask it for previews as well.
    // This is a little wonky because the preview prompter just spits out a
    // static string anyway, but it's "technically" right and plays well in
    // tests. Also it reminds users that a prompt is sensitive in the TUI :)
    if is_sensitive {
        Ok(sensitive((&context, output)))
    } else {
        Ok(output)
    }
}

/// Keyword args for both [response] and [response_headers]
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResponseKwargs {
    /// If/when should we trigger an upstream request?
    #[serde(default)]
    trigger: RequestTrigger,
}

/// Load the most recent response body for a recipe and the current profile
async fn response(
    (context, recipe_id, Kwargs(ResponseKwargs { trigger })): (
        &TemplateContext,
        RecipeId,
        Kwargs<ResponseKwargs>,
    ),
) -> Result<Vec<u8>, FunctionError> {
    let response = context
        .get_latest_response(process, &recipe_id, trigger)
        .await?;
    let body = match Arc::try_unwrap(response) {
        Ok(response) => response.body,
        Err(response) => response.body.clone(),
    };
    body.into_bytes()
}

/// Load a header value from the most recent response for a recipe and the
/// current profile
async fn response_header(
    (context, recipe_id, header, Kwargs(ResponseKwargs { trigger })): (
        &TemplateContext,
        RecipeId,
        String,
        Kwargs<ResponseKwargs>,
    ),
) -> Result<Vec<u8>, FunctionError> {
    let response = context.get_latest_response(&recipe_id, trigger).await?;
    // Only clone the header value if necessary
    let header_value = match Arc::try_unwrap(response) {
        Ok(mut response) => response.headers.remove(&header),
        Err(response) => response.headers.get(&header).cloned(),
    }
    .ok_or_else(|| FunctionError::ResponseMissingHeader { header })?;
    // HeaderValue doesn't expose any way to move its bytes out so we must clone
    // https://github.com/hyperium/http/issues/661
    header_value.as_bytes().to_vec()
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct SelectKwargs {
    message: Option<String>,
}

/// Ask the user to select a value from a list
async fn select(
    (context, options, Kwargs(SelectKwargs { message })): (
        &TemplateContext,
        Vec<String>,
        Kwargs<SelectKwargs>,
    ),
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
fn sensitive((context, value): (&TemplateContext, String)) -> String {
    if context.show_sensitive {
        value
    } else {
        "•".repeat(value.chars().count())
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
                            v.parse::<Duration>().map_err(de::Error::custom)?;
                        Ok(RequestTrigger::Expire { duration })
                    }
                }
            }
        }

        deserializer.deserialize_any(RequestTriggerVisitor)
    }
}
