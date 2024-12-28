//! HCL functions provided to users, to be used in collections

use crate::{
    collection::{RecipeId, RequestTrigger},
    template::{
        error::RenderResultExt,
        render::{
            bytes_to_value, value_to_string, RenderResult, RenderValue,
            TrimMode,
        },
        Prompt, RenderContext, RenderError, Select,
    },
};
use serde::{de::DeserializeOwned, Deserialize};
use serde_json_path::JsonPath;
use std::{path::PathBuf, process::Stdio};
use tokio::{fs, io::AsyncWriteExt, process::Command, sync::oneshot};
use tracing::{debug, debug_span};

/// Call the function with the given name, deserializing the given expression
/// into the arguments expected by the function. All functions take keywords
/// arguments, i.e. they expect a single object argument
pub async fn call_fn(
    name: &str,
    arg: RenderValue,
    context: &RenderContext,
) -> RenderResult {
    // Helper macro to generate a match statement for all functions, so we can
    // do static dispatch. This keeps the type signatures simpler on each fn
    macro_rules! fns {
        ($($func:ty),* $(,)?) => {
            match name {
                $(
                    <$func>::NAME => {
                        map_args::<$func>(arg)?
                            .call(context)
                            .await
                    }
                )*
                _ => Err(RenderError::FunctionUnknown {
                    name: name.to_string(),
                }),
            }
        }
    }

    fns!(
        // All accessible functions are listed here
        CommandFn,
        EnvFn,
        FileFn,
        JsonPathFn,
        PromptFn,
        ResponseFn,
        ResponseHeaderFn,
        SelectFn,
        ToStringFn,
    )
}

fn map_args<T: HclFunction>(arg: RenderValue) -> Result<T, RenderError> {
    // TODO func arg deserialization should get a special error variant
    hcl::from_value::<T, _>(arg)
        .map_err(|error| RenderError::FunctionArgument {
            error: error.into(),
        })
        .context(format!("arguments to function `{}`", T::NAME))
}

/// A Rust function exposed to HCL. This trait defines the name, arguments,
/// and implementation of the function. Each function gets deserialized from an
/// HCL value into its args, hence the `Deserialize` bound
pub trait HclFunction: DeserializeOwned {
    const NAME: &'static str;

    async fn call(self, context: &RenderContext) -> RenderResult;
}

/// Run a command in a subprocess
/// TODO accept command+args in one field?
#[derive(Deserialize)]
struct CommandFn {
    /// Name/path to the command
    command: String,
    /// Arguments to pass to the command
    #[serde(default)]
    args: Vec<String>,
    /// Optional data to pipe to the command via stdin
    stdin: Option<String>,
    /// Trim whitespace from the start/end of command output
    #[serde(default)]
    trim: TrimMode,
}

impl HclFunction for CommandFn {
    const NAME: &'static str = "command";

    async fn call(self, _: &RenderContext) -> RenderResult {
        let Self {
            command: program,
            args,
            stdin,
            trim,
        } = self;
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
        .map_err(|error| RenderError::Command {
            program,
            args,
            error: error.into(),
        })?;

        debug!(
            stdout = %String::from_utf8_lossy(&output.stdout),
            stderr = %String::from_utf8_lossy(&output.stderr),
            "Command success"
        );

        let trimmed = trim.apply(output.stdout);
        Ok(bytes_to_value(trimmed))
    }
}

/// Load the value of an environment variable
#[derive(Deserialize)]
struct EnvFn {
    variable: String,
}

impl HclFunction for EnvFn {
    const NAME: &'static str = "env";

    async fn call(self, _: &RenderContext) -> RenderResult {
        Ok(std::env::var(&self.variable).unwrap_or_default().into())
    }
}

/// Load the contents of a file
#[derive(Deserialize)]
struct FileFn {
    path: PathBuf,
}

impl HclFunction for FileFn {
    const NAME: &'static str = "file";

    async fn call(self, _: &RenderContext) -> RenderResult {
        let output =
            fs::read(&self.path)
                .await
                .map_err(|error| RenderError::File {
                    path: self.path,
                    error: error.into(),
                })?;
        Ok(output.into())
    }
}

/// Query a JSON string via JSONPath. This always outputs an array, so consumers
/// will have to grab the first element manually
#[derive(Deserialize)]
struct JsonPathFn {
    query: String,
    data: String,
}

impl HclFunction for JsonPathFn {
    const NAME: &'static str = "json_path";

    async fn call(self, _: &RenderContext) -> RenderResult {
        fn json_to_hcl(json: &serde_json::Value) -> RenderValue {
            match json {
                serde_json::Value::Null => RenderValue::Null,
                serde_json::Value::Bool(b) => RenderValue::Bool(*b),
                serde_json::Value::Number(number) => {
                    RenderValue::Number(todo!())
                }
                serde_json::Value::String(s) => RenderValue::String(s.clone()),
                serde_json::Value::Array(vec) => {
                    RenderValue::Array(vec.iter().map(json_to_hcl).collect())
                }
                serde_json::Value::Object(map) => RenderValue::Object(
                    map.iter()
                        .map(|(key, value)| (key.clone(), json_to_hcl(value)))
                        .collect(),
                ),
            }
        }

        let query = JsonPath::parse(&self.query).map_err(|error| {
            RenderError::JsonPathParse {
                path: self.query,
                error: error.into(),
            }
        })?;
        let json: serde_json::Value = serde_json::from_str(&self.data)
            .map_err(|error| RenderError::ResponseParse {
                error: error.into(),
            })?;

        let node_list = query.query(&json);
        Ok(RenderValue::Array(
            node_list.into_iter().map(json_to_hcl).collect(),
        ))

        // TODO use selector mode?
        /*
        /// Stringify a single JSON value into this format
        fn value_to_string(value: &serde_json::Value) -> String {
            match value {
                serde_json::Value::Null => "".into(),
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            }
        }

        /// Stringify a list of JSON values into this format
        fn vec_to_string(values: &Vec<&serde_json::Value>) -> String {
            serde_json::to_string(&values).unwrap()
        }

        let stringified = match self.mode {
            SelectorMode::Auto => match node_list.len() {
                0 => return Err(RenderError::JsonPathNone),
                1 => value_to_string(node_list.first().unwrap()),
                2.. => vec_to_string(&node_list.all()),
            },
            SelectorMode::Single => {
                let value =
                    node_list.exactly_one().map_err(|error| match error {
                        ExactlyOneError::Empty => RenderError::JsonPathNone,
                        ExactlyOneError::MoreThanOne(n) => {
                            RenderError::JsonPathTooMany { actual_count: n }
                        }
                    })?;
                value_to_string(value)
            }
            SelectorMode::Array => vec_to_string(&node_list.all()),
        };

        Ok(stringified.into())
        */
    }
}

/// Prompt the user to enter a text value
#[derive(Deserialize)]
struct PromptFn {
    message: String,
    default: Option<String>,
    #[serde(default)]
    sensitive: bool,
}

impl HclFunction for PromptFn {
    const NAME: &'static str = "prompt";

    async fn call(self, context: &RenderContext) -> RenderResult {
        let (tx, rx) = oneshot::channel();
        context.prompter.prompt(Prompt {
            message: self.message,
            default: self.default,
            sensitive: self.sensitive,
            channel: tx.into(),
        });
        let output = rx.await.map_err(|_| RenderError::PromptNoReply)?;
        Ok(output.into())
    }
}

/// Load the most recent response body for a recipe and the
/// current profile
#[derive(Deserialize)]
struct ResponseFn {
    recipe: RecipeId,
    #[serde(default)]
    trigger: RequestTrigger,
}

impl HclFunction for ResponseFn {
    const NAME: &'static str = "response";

    async fn call(self, context: &RenderContext) -> RenderResult {
        let response = context
            .get_latest_response(&self.recipe, self.trigger)
            .await?;
        Ok(bytes_to_value(response.body.into_bytes().into()))
    }
}

/// Load a header value from the most recent response for a
/// recipe and the current profile
#[derive(Deserialize)]
struct ResponseHeaderFn {
    recipe: RecipeId,
    header: String,
    #[serde(default)]
    trigger: RequestTrigger,
}

impl HclFunction for ResponseHeaderFn {
    const NAME: &'static str = "response_header";

    async fn call(self, context: &RenderContext) -> RenderResult {
        let mut response = context
            .get_latest_response(&self.recipe, self.trigger)
            .await?;
        let header =
            response.headers.remove(&self.header).ok_or_else(|| {
                RenderError::ResponseMissingHeader {
                    header: self.header.clone(),
                }
            })?;
        // TODO respect is_sensitive flag?
        // HeaderValue doesn't expose its inner bytes so we have to clone :(
        Ok(header.as_bytes().to_owned().into())
    }
}

/// Ask the user to select a value from a list
#[derive(Deserialize)]
struct SelectFn {
    message: String,
    options: Vec<String>,
}

impl HclFunction for SelectFn {
    const NAME: &'static str = "select";

    async fn call(self, context: &RenderContext) -> RenderResult {
        let (tx, rx) = oneshot::channel();
        context.prompter.select(Select {
            message: self.message,
            options: self.options,
            channel: tx.into(),
        });
        let output = rx.await.map_err(|_| RenderError::PromptNoReply)?;
        Ok(output.into())
    }
}

/// Coerce any value to a string, so it can be used as input for a string thing
///
/// This uses the name `tostring` to match Terraform. `to_string` would be
/// better style, but predictability is probably worth more.
#[derive(Deserialize)]
#[serde(transparent)]
struct ToStringFn(RenderValue);

impl HclFunction for ToStringFn {
    const NAME: &'static str = "tostring";

    async fn call(self, _: &RenderContext) -> RenderResult {
        Ok(value_to_string(self.0).into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_json_path() {
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

    #[test]
    fn test_select() {
        todo!()
    }

    #[test]
    fn test_to_string() {
        todo!()
    }
}
