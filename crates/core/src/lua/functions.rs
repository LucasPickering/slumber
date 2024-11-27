//! Lua functions provided to users, to be used in collections

use crate::{
    collection::{RecipeId, RequestTrigger, TrimMode},
    lua::{FunctionError, LuaRenderer, LuaWrap},
    template::{Prompt, Select},
};
use bytes::Bytes;
use mlua::{IntoLua, Table};
use reqwest::header::HeaderValue;
use serde::{de::DeserializeOwned, Deserialize};
use serde_json_path::JsonPath;
use std::{future::Future, path::PathBuf, process::Stdio};
use tokio::{fs, io::AsyncWriteExt, process::Command, sync::oneshot};
use tracing::{debug, debug_span};

/// A Rust function exposed to Lua. This trait defines the name, return type,
/// and implementation of the function.
pub trait LuaFunction: 'static + Send + DeserializeOwned {
    const NAME: &'static str;
    type Output: IntoLua;

    fn call(
        self,
        _: LuaRenderer,
    ) -> impl 'static + Future<Output = Result<Self::Output, FunctionError>> + Send;

    /// Generate Lua source code to call this function with the contained args.
    /// Useful for collection importers.
    fn to_source(&self) -> String {
        let name = Self::NAME;
        format!("{name}({{ TODO }})")
    }
}

/// Ask the user to select a value from a list. Previously this was the `select`
/// chain source, but `select` is a built-in function in Lua.
#[derive(Deserialize)]
pub struct ChooseFn {
    pub message: String,
    pub options: Vec<String>,
}

impl LuaFunction for ChooseFn {
    const NAME: &'static str = "choose";
    type Output = String;

    async fn call(
        self,
        renderer: LuaRenderer,
    ) -> Result<Self::Output, FunctionError> {
        let (tx, rx) = oneshot::channel();
        renderer.context().prompter.select(Select {
            message: self.message,
            options: self.options,
            channel: tx.into(),
        });
        let output = rx.await.map_err(|_| FunctionError::PromptNoReply)?;
        Ok(output)
    }
}

/// Run a command in a subprocess
#[derive(Deserialize)]
pub struct CommandFn {
    /// Name/path to the command
    pub command: String,
    /// Arguments to pass to the command
    #[serde(default)]
    pub args: Vec<String>,
    /// Optional data to pipe to the command via stdin
    pub stdin: Option<String>,
    /// Trim whitespace from beginning/end of output
    pub trim: TrimMode,
}

impl LuaFunction for CommandFn {
    const NAME: &'static str = "command";
    type Output = LuaWrap<Vec<u8>>;

    async fn call(self, _: LuaRenderer) -> Result<Self::Output, FunctionError> {
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
        Ok(trimmed.into())
    }
}

/// Load the value of an environment variable
#[derive(Deserialize)]
pub struct EnvFn {
    pub variable: String,
}

impl LuaFunction for EnvFn {
    const NAME: &'static str = "env";
    type Output = String;

    async fn call(self, _: LuaRenderer) -> Result<Self::Output, FunctionError> {
        Ok(std::env::var(&self.variable).unwrap_or_default())
    }
}

/// Load the contents of a file
#[derive(Deserialize)]
pub struct FileFn {
    pub path: PathBuf,
}

impl LuaFunction for FileFn {
    const NAME: &'static str = "file";
    type Output = LuaWrap<Vec<u8>>;

    async fn call(self, _: LuaRenderer) -> Result<Self::Output, FunctionError> {
        let output = fs::read(&self.path).await.map_err(|error| {
            FunctionError::File {
                path: self.path,
                error,
            }
        })?;
        Ok(output.into())
    }
}

/// Query a JSON string via JSONPath
#[derive(Deserialize)]
pub struct JsonPathFn {
    query: String,
    data: String,
}

impl LuaFunction for JsonPathFn {
    const NAME: &'static str = "json_path";
    type Output = String;

    async fn call(self, _: LuaRenderer) -> Result<Self::Output, FunctionError> {
        let query = JsonPath::parse(&self.query).map_err(|error| {
            FunctionError::JsonPathParse {
                path: self.query,
                source: error,
            }
        })?;
        let json: serde_json::Value = serde_json::from_str(&self.data)
            .map_err(|error| FunctionError::ResponseParse { error })?;
        let output = query.query(&json).exactly_one()?;
        let stringified = match output {
            serde_json::Value::Null => "".into(),
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        Ok(stringified)
    }
}

/// Get the template-rendered data for the currently selected profile
#[derive(Deserialize)]
pub struct ProfileFn;

impl LuaFunction for ProfileFn {
    const NAME: &'static str = "profile";
    type Output = Table;

    async fn call(
        self,
        renderer: LuaRenderer,
    ) -> Result<Self::Output, FunctionError> {
        // We should only initialize the profile once, so it's wrapped in a
        // mutex. Subsequent calls on the same recipe will block here until the
        // first call is done
        let mut table = renderer.context().state.profile_data.lock().await;
        if let Some(table) = &*table {
            // Cloning is cheap because it's just a pointer into lua
            Ok(table.clone())
        } else {
            let t = renderer.render_profile().await?;
            *table = Some(t.clone());
            Ok(t)
        }
    }
}

/// Prompt the user to enter a text value
#[derive(Deserialize)]
pub struct PromptFn {
    pub message: String,
    pub default: Option<String>,
    #[serde(default)]
    pub sensitive: bool,
}

impl LuaFunction for PromptFn {
    const NAME: &'static str = "prompt";
    type Output = String;

    async fn call(
        self,
        renderer: LuaRenderer,
    ) -> Result<Self::Output, FunctionError> {
        let (tx, rx) = oneshot::channel();
        renderer.context().prompter.prompt(Prompt {
            message: self.message,
            default: self.default,
            sensitive: self.sensitive,
            channel: tx.into(),
        });
        let output = rx.await.map_err(|_| FunctionError::PromptNoReply)?;
        Ok(output)
    }
}

/// Load the most recent response body for a recipe and the
/// current profile
#[derive(Deserialize)]
pub struct ResponseArgs {
    pub recipe: RecipeId,
    #[serde(default)]
    pub trigger: RequestTrigger,
}

impl LuaFunction for ResponseArgs {
    const NAME: &'static str = "response";
    type Output = LuaWrap<Bytes>;

    async fn call(
        self,
        renderer: LuaRenderer,
    ) -> Result<Self::Output, FunctionError> {
        let response = renderer
            .get_latest_response(&self.recipe, self.trigger)
            .await?;
        Ok(response.body.into_bytes().into())
    }
}

/// Load a header value from the most recent response for a
/// recipe and the current profile
#[derive(Deserialize)]
pub struct ResponseHeaderArgs {
    pub recipe: RecipeId,
    pub header: String,
    #[serde(default)]
    pub trigger: RequestTrigger,
}

impl LuaFunction for ResponseHeaderArgs {
    const NAME: &'static str = "response_header";
    type Output = LuaWrap<HeaderValue>;

    async fn call(
        self,
        renderer: LuaRenderer,
    ) -> Result<Self::Output, FunctionError> {
        let mut response = renderer
            .get_latest_response(&self.recipe, self.trigger)
            .await?;
        let header =
            response.headers.remove(&self.header).ok_or_else(|| {
                FunctionError::ResponseMissingHeader {
                    header: self.header.clone(),
                }
            })?;
        Ok(header.into())
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
}
