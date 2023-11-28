//! Template rendering implementation

use crate::{
    collection::{ChainSource, ProfileValue, RequestRecipeId},
    http::{ContentType, Json},
    template::{
        parse::TemplateInputChunk, ChainError, Prompt, Template, TemplateChunk,
        TemplateContext, TemplateError, TemplateKey,
    },
    util::ResultExt,
};
use async_trait::async_trait;
use futures::future::join_all;
use serde_json_path::JsonPath;
use std::{
    env::{self},
    ops::Deref as _,
    path::Path,
};
use tokio::{fs, process::Command, sync::oneshot};
use tracing::{info, instrument, trace};

/// Outcome of rendering a single chunk. This allows attaching some metadata to
/// the render.
#[derive(Debug)]
struct RenderedChunk {
    value: String,
    sensitive: bool,
}

type TemplateResult = Result<RenderedChunk, TemplateError>;

impl Template {
    /// Render the template string using values from the given context. If an
    /// error occurs, it is returned as general `anyhow` error. If you need a
    /// more specific error, use [Self::render_borrow].
    pub async fn render(
        &self,
        context: &TemplateContext,
    ) -> anyhow::Result<String> {
        self.render_stitched(context)
            .await
            .map_err(anyhow::Error::from)
            .traced()
    }

    /// Render the template string using values from the given context,
    /// returning the individual rendered chunks. This is useful in any
    /// application where rendered chunks need to be handled differently from
    /// raw chunks, e.g. in render previews.
    #[instrument(skip_all, fields(template = self.template))]
    pub async fn render_chunks(
        &self,
        context: &TemplateContext,
    ) -> Vec<TemplateChunk> {
        // Map over each parsed chunk, and render the keys into strings. The
        // raw text chunks will be mapped 1:1
        let futures = self.chunks.iter().copied().map(|chunk| async move {
            match chunk {
                TemplateInputChunk::Raw(span) => TemplateChunk::Raw(span),
                TemplateInputChunk::Key(key) => {
                    // Grab the string corresponding to the span
                    let key = key.map(|span| self.substring(span));

                    // The formatted key should match the source that it was
                    // parsed from, therefore we can use it to match the
                    // override key
                    let raw = key.to_string();
                    // If the key is in the overrides, use the given value
                    // without parsing it
                    let result = match context.overrides.get(&raw) {
                        Some(value) => {
                            trace!(
                                key = raw,
                                value,
                                "Rendered template key from override"
                            );
                            Ok(RenderedChunk {
                                value: value.clone(),
                                // The overriden value *could* be marked
                                // sensitive, but we're taking a shortcut and
                                // assuming it isn't
                                sensitive: false,
                            })
                        }
                        None => {
                            // Standard case - parse the key and render it
                            let result =
                                key.into_source().render(context).await;
                            if let Ok(value) = &result {
                                trace!(
                                    key = raw,
                                    ?value,
                                    "Rendered template key"
                                );
                            }
                            result
                        }
                    };
                    result.into()
                }
            }
        });

        // Parallelization!
        join_all(futures).await
    }

    /// Helper for stitching chunks together into a single string. If any chunk
    /// failed to render, return an error.
    pub(super) async fn render_stitched(
        &self,
        context: &TemplateContext,
    ) -> Result<String, TemplateError> {
        // Render each individual template chunk in the string
        let chunks = self.render_chunks(context).await;

        // Stitch the rendered chunks together into one string
        let mut buffer = String::with_capacity(self.len());
        for chunk in chunks {
            match chunk {
                TemplateChunk::Raw(span) => {
                    buffer.push_str(self.substring(span));
                }
                TemplateChunk::Rendered { value, .. } => {
                    buffer.push_str(&value)
                }
                TemplateChunk::Error(error) => return Err(error),
            }
        }
        Ok(buffer)
    }
}

impl From<TemplateResult> for TemplateChunk {
    fn from(result: TemplateResult) -> Self {
        match result {
            Ok(outcome) => Self::Rendered {
                value: outcome.value,
                sensitive: outcome.sensitive,
            },
            Err(error) => Self::Error(error),
        }
    }
}

impl<'a> TemplateKey<&'a str> {
    /// Convert this key into a renderable value type
    fn into_source(self) -> Box<dyn TemplateSource<'a>> {
        match self {
            Self::Field(field) => Box::new(FieldTemplateSource { field }),
            Self::Chain(chain_id) => Box::new(ChainTemplateSource { chain_id }),
            Self::Environment(variable) => {
                Box::new(EnvironmentTemplateSource { variable })
            }
        }
    }
}

/// A single-type parsed template key, which can be rendered into a string.
/// This should be one implementation of this for each variant of [TemplateKey].
///
/// By breaking `TemplateKey` apart into multiple types, we can split the
/// render logic easily amongst a bunch of functions. It's not technically
/// necessary, just a code organization thing.
#[async_trait]
trait TemplateSource<'a>: 'a + Send + Sync {
    /// Render this intermediate value into a string. Return a Cow because
    /// sometimes this can be a reference to the template context, but
    /// other times it has to be owned data (e.g. when pulling response data
    /// from the repository).
    async fn render(&self, context: &'a TemplateContext) -> TemplateResult;
}

/// A simple field value (e.g. from the profile or an override)
struct FieldTemplateSource<'a> {
    pub field: &'a str,
}

#[async_trait]
impl<'a> TemplateSource<'a> for FieldTemplateSource<'a> {
    async fn render(&self, context: &'a TemplateContext) -> TemplateResult {
        let field = self.field;
        let value = context.profile.get(field).ok_or_else(|| {
            TemplateError::FieldUnknown {
                field: field.to_owned(),
            }
        })?;
        let rendered = match value {
            ProfileValue::Raw(value) => value.clone(),
            // recursion!
            ProfileValue::Template(template) => {
                trace!(%field, %template, "Rendering recursive template");
                template.render_stitched(context).await.map_err(|error| {
                    TemplateError::Nested {
                        template: template.clone(),
                        error: Box::new(error),
                    }
                })?
            }
        };
        Ok(RenderedChunk {
            value: rendered,
            sensitive: false,
        })
    }
}

/// A chained value from a complex source. Could be an HTTP response, file, etc.
struct ChainTemplateSource<'a> {
    pub chain_id: &'a str,
}

#[async_trait]
impl<'a> TemplateSource<'a> for ChainTemplateSource<'a> {
    async fn render(&self, context: &'a TemplateContext) -> TemplateResult {
        let chain_id = self.chain_id;

        // Any error in here is the chain error subtype
        let result: Result<_, ChainError> = async {
            // Resolve chained value
            let chain = context
                .chains
                .iter()
                .find(|chain| chain.id.deref() == chain_id)
                .ok_or(ChainError::Unknown)?;

            // Resolve the value based on the source type
            let value = match &chain.source {
                ChainSource::Request(recipe_id) => {
                    self.render_request(context, recipe_id).await?
                }
                ChainSource::File(path) => self.render_file(path).await?,
                ChainSource::Command(command) => {
                    self.render_command(command).await?
                }
                ChainSource::Prompt(label) => {
                    self.render_prompt(
                        context,
                        label.as_deref(),
                        chain.sensitive,
                    )
                    .await?
                }
            };

            // If a selector path is present, filter down the value
            let value = match &chain.selector {
                Some(path) => self.apply_selector(&value, path)?,
                None => value,
            };

            Ok(RenderedChunk {
                value,
                sensitive: chain.sensitive,
            })
        }
        .await;

        // Wrap the chain error into a TemplateError
        result.map_err(|error| TemplateError::Chain {
            chain_id: chain_id.into(),
            error,
        })
    }
}

impl<'a> ChainTemplateSource<'a> {
    /// Render a chained template value from a response
    async fn render_request(
        &self,
        context: &'a TemplateContext,
        recipe_id: &RequestRecipeId,
    ) -> Result<String, ChainError> {
        let record = context
            .repository
            .get_last(recipe_id)
            .await
            .map_err(ChainError::Repository)?
            .ok_or(ChainError::NoResponse)?;

        Ok(record.response.body.into_text())
    }

    /// Render a chained value from a file
    async fn render_file(&self, path: &'a Path) -> Result<String, ChainError> {
        fs::read_to_string(path)
            .await
            .map_err(|error| ChainError::File {
                path: path.to_owned(),
                error,
            })
    }

    /// Render a chained value from an external command
    async fn render_command(
        &self,
        command: &[String],
    ) -> Result<String, ChainError> {
        match command {
            [] => Err(ChainError::CommandMissing),
            [program, args @ ..] => {
                let output =
                    Command::new(program).args(args).output().await.map_err(
                        |error| ChainError::Command {
                            command: command.to_owned(),
                            error,
                        },
                    )?;
                info!(
                    ?command,
                    stdout = %String::from_utf8_lossy(&output.stdout),
                    stderr = %String::from_utf8_lossy(&output.stderr),
                    "Executing subcommand"
                );
                String::from_utf8(output.stdout).map_err(|error| {
                    ChainError::CommandInvalidUtf8 {
                        command: command.to_owned(),
                        error,
                    }
                })
            }
        }
    }

    /// Render a value by asking the user to provide it
    async fn render_prompt(
        &self,
        context: &'a TemplateContext,
        label: Option<&str>,
        sensitive: bool,
    ) -> Result<String, ChainError> {
        // Use the prompter to ask the user a question, and wait for a response
        // on the prompt channel
        let (tx, rx) = oneshot::channel();
        context.prompter.prompt(Prompt {
            label: label.unwrap_or(self.chain_id).into(),
            sensitive,
            channel: tx,
        });
        rx.await.map_err(|_| ChainError::PromptNoResponse)
    }

    /// Apply a selector path to a string value to filter it down. Right now
    /// this only supports JSONpath but we could add support for more in the
    /// future. The string value will be parsed as a JSON value.
    fn apply_selector(
        &self,
        value: &str,
        selector: &JsonPath,
    ) -> Result<String, ChainError> {
        // Parse the response as JSON. Intentionally ignore the
        // content-type. If the user wants to treat it as JSON, we
        // should allow that even if the server is wrong.
        let json_value = Json::parse(value)
            .map_err(|err| ChainError::ParseResponse { error: err })?;

        // Apply the path to the json
        let found_value = selector
            .query(&json_value)
            .exactly_one()
            .map_err(|err| ChainError::InvalidResult { error: err })?;

        match found_value {
            serde_json::Value::String(s) => Ok(s.clone()),
            other => Ok(other.to_string()),
        }
    }
}

/// A value sourced from the process's environment
struct EnvironmentTemplateSource<'a> {
    pub variable: &'a str,
}

#[async_trait]
impl<'a> TemplateSource<'a> for EnvironmentTemplateSource<'a> {
    async fn render(&self, _: &'a TemplateContext) -> TemplateResult {
        let value = env::var(self.variable).map_err(|err| {
            TemplateError::EnvironmentVariable {
                variable: self.variable.to_owned(),
                error: err,
            }
        })?;
        Ok(RenderedChunk {
            value,
            sensitive: false,
        })
    }
}
