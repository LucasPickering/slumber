//! Template rendering implementation

use crate::{
    collection::{
        ChainId, ChainOutputTrim, ChainRequestSection, ChainRequestTrigger,
        ChainSource, RecipeId,
    },
    http::{ContentType, Exchange, RequestSeed, ResponseRecord},
    template::{
        error::TriggeredRequestError, parse::TemplateInputChunk, ChainError,
        Prompt, Template, TemplateChunk, TemplateContext, TemplateError,
        TemplateKey, RECURSION_LIMIT,
    },
    util::ResultExt,
};
use async_trait::async_trait;
use chrono::Utc;
use futures::future;
use std::{
    env,
    path::PathBuf,
    process::Stdio,
    sync::{atomic::Ordering, Arc},
};
use tokio::{fs, io::AsyncWriteExt, process::Command, sync::oneshot};
use tracing::{debug, debug_span, instrument, trace};

/// Outcome of rendering a single chunk. This allows attaching some metadata to
/// the render.
#[derive(Debug)]
struct RenderedChunk {
    value: Vec<u8>,
    sensitive: bool,
}

type TemplateResult = Result<RenderedChunk, TemplateError>;

impl Template {
    /// Render the template using values from the given context. If any chunk
    /// failed to render, return an error. The template is rendered as bytes.
    /// Use [Self::render_string] if you want the bytes converted to a string.
    pub async fn render(
        &self,
        context: &TemplateContext,
    ) -> Result<Vec<u8>, TemplateError> {
        debug!(template = %self, "Rendering template");

        if context.recursion_count.load(Ordering::Relaxed) >= RECURSION_LIMIT {
            return Err(TemplateError::RecursionLimit);
        }

        // Render each individual template chunk in the string
        let chunks = self.render_chunks(context).await;

        // Stitch the chunks together into one buffer
        let len = chunks
            .iter()
            .map(|chunk| match chunk {
                TemplateChunk::Raw(text) => text.as_bytes().len(),
                TemplateChunk::Rendered { value, .. } => value.len(),
                TemplateChunk::Error(_) => 0,
            })
            .sum();
        let mut buf = Vec::with_capacity(len);
        for chunk in chunks {
            match chunk {
                TemplateChunk::Raw(text) => buf.extend(text.as_bytes()),
                TemplateChunk::Rendered { value, .. } => buf.extend(value),
                TemplateChunk::Error(error) => return Err(error),
            }
        }

        Ok(buf)
    }

    /// Render the template using values from the given context. If any chunk
    /// failed to render, return an error. The rendered template will be
    /// converted from raw bytes to UTF-8. If it is not valid UTF-8, return an
    /// error.
    pub async fn render_string(
        &self,
        context: &TemplateContext,
    ) -> Result<String, TemplateError> {
        let bytes = self.render(context).await?;
        String::from_utf8(bytes).map_err(TemplateError::InvalidUtf8)
    }

    /// Render the template string using values from the given context,
    /// returning the individual rendered chunks. This is useful in any
    /// application where rendered chunks need to be handled differently from
    /// raw chunks, e.g. in render previews.
    #[instrument(skip_all, fields(template = %self))]
    pub async fn render_chunks(
        &self,
        context: &TemplateContext,
    ) -> Vec<TemplateChunk> {
        // Map over each parsed chunk, and render the keys into strings. The
        // raw text chunks will be mapped 1:1. This clone is pretty cheap
        // because raw text uses Arc and keys just contain metadata
        let futures = self.chunks.iter().cloned().map(|chunk| async move {
            match chunk {
                TemplateInputChunk::Raw(text) => TemplateChunk::Raw(text),
                TemplateInputChunk::Key(key) => {
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
                                value: value.clone().into_bytes(),
                                // The overriden value *could* be marked
                                // sensitive, but we're taking a shortcut and
                                // assuming it isn't
                                sensitive: false,
                            })
                        }
                        None => {
                            // Standard case - parse the key and render it
                            let result = key.to_source().render(context).await;
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
        future::join_all(futures).await
    }

    /// Render a template whose result will be used as configuration for a
    /// chain. It's assumed we need string output for that. The given field name
    /// will be used to provide a descriptive error.
    async fn render_nested(
        &self,
        field: impl Into<String>,
        context: &TemplateContext,
    ) -> Result<String, ChainError> {
        self.render_string(context)
            .await
            .map_err(|error| ChainError::Nested {
                field: field.into(),
                error: error.into(),
            })
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

impl TemplateKey {
    /// Convert this key into a renderable value type
    fn to_source(&self) -> Box<dyn '_ + TemplateSource<'_>> {
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
    /// from the database).
    async fn render(&self, context: &'a TemplateContext) -> TemplateResult;
}

/// A simple field value (e.g. from the profile or an override)
struct FieldTemplateSource<'a> {
    field: &'a str,
}

#[async_trait]
impl<'a> TemplateSource<'a> for FieldTemplateSource<'a> {
    async fn render(&self, context: &'a TemplateContext) -> TemplateResult {
        let field = self.field;

        // Get the value from the profile
        let profile_id = context
            .selected_profile
            .as_ref()
            .ok_or_else(|| TemplateError::NoProfileSelected)?;
        // Typically the caller should validate the ID is valid, this is just
        // a backup check
        let profile =
            context.collection.profiles.get(profile_id).ok_or_else(|| {
                TemplateError::ProfileUnknown {
                    profile_id: profile_id.clone(),
                }
            })?;
        let template = profile.data.get(field).ok_or_else(|| {
            TemplateError::FieldUnknown {
                field: field.to_owned(),
            }
        })?;

        // recursion!
        trace!(%field, %template, "Rendering recursive template");
        context.recursion_count.fetch_add(1, Ordering::Relaxed);
        let rendered = template.render(context).await.map_err(|error| {
            TemplateError::FieldNested {
                field: field.to_owned(),
                error: Box::new(error),
            }
        })?;
        Ok(RenderedChunk {
            value: rendered,
            sensitive: false,
        })
    }
}

/// A chained value from a complex source. Could be an HTTP response, file, etc.
struct ChainTemplateSource<'a> {
    chain_id: &'a ChainId,
}

#[async_trait]
impl<'a> TemplateSource<'a> for ChainTemplateSource<'a> {
    async fn render(&self, context: &'a TemplateContext) -> TemplateResult {
        // Any error in here is the chain error subtype
        let result: Result<_, ChainError> = async {
            // Resolve chained value
            let chain =
                context.collection.chains.get(self.chain_id).ok_or_else(
                    || ChainError::ChainUnknown(self.chain_id.clone()),
                )?;

            // Resolve the value based on the source type. Also resolve its
            // content type. For responses this will come from its header, from
            // files from its extension. For anything else, we'll fall back to
            // the content_type field defined by the user.
            //
            // We intentionally throw the content detection error away here,
            // because it isn't that intuitive for users and is hard to plumb
            let (value, content_type) = match &chain.source {
                ChainSource::Command { command, stdin } => (
                    self.render_command(context, command, stdin.as_ref())
                        .await?,
                    // No way to guess content type on this
                    None,
                ),
                ChainSource::File { path } => {
                    self.render_file(context, path).await?
                }
                ChainSource::Environment { variable } => (
                    self.render_environment_variable(context, variable).await?,
                    // No way to guess content type on this
                    None,
                ),
                ChainSource::Prompt { message, default } => (
                    self.render_prompt(
                        context,
                        message.as_ref(),
                        default.as_ref(),
                        chain.sensitive,
                    )
                    .await?
                    .into_bytes(),
                    // No way to guess content type on this
                    None,
                ),
                ChainSource::Request {
                    recipe,
                    trigger,
                    section,
                } => {
                    let response =
                        self.get_response(context, recipe, *trigger).await?;
                    // Guess content type based on HTTP header
                    let content_type =
                        ContentType::from_response(&response).ok();
                    let value =
                        self.extract_response_value(response, section)?;
                    (value, content_type)
                }
            };
            // If the user provided a content type, prefer that over the
            // detected one
            let content_type = chain.content_type.or(content_type);

            // If a selector path is present, filter down the value
            let value = if let Some(selector) = &chain.selector {
                let content_type =
                    content_type.ok_or(ChainError::UnknownContentType)?;
                // Parse according to detected content type
                let value = content_type
                    .parse_content(&value)
                    .map_err(|err| ChainError::ParseResponse { error: err })?;
                selector.query_to_string(&*value)?.into_bytes()
            } else {
                value
            };

            Ok(RenderedChunk {
                value: chain.trim.apply(value),
                sensitive: chain.sensitive,
            })
        }
        .await;

        // Wrap the chain error into a TemplateError
        result.map_err(|error| TemplateError::Chain {
            chain_id: self.chain_id.clone(),
            error,
        })
    }
}

impl<'a> ChainTemplateSource<'a> {
    /// Get an HTTP response for a recipe. This will either get the most recent
    /// response from history or re-execute the request, depending on trigger
    /// behavior.
    async fn get_response(
        &self,
        context: &'a TemplateContext,
        recipe_id: &RecipeId,
        trigger: ChainRequestTrigger,
    ) -> Result<ResponseRecord, ChainError> {
        // Get the referenced recipe. We actually only need the whole recipe if
        // we're executing the request, but we want this to error out if the
        // recipe doesn't exist regardless. It's possible the recipe isn't in
        // the collection but still exists in history (if it was deleted).
        // Eagerly checking for it makes that case error out, which is more
        // intuitive than using history for a deleted recipe.
        let recipe = context
            .collection
            .recipes
            .get_recipe(recipe_id)
            .ok_or_else(|| ChainError::RecipeUnknown(recipe_id.clone()))?;

        // Defer loading the most recent exchange until we know we'll need it
        let get_most_recent = || -> Result<Option<Exchange>, ChainError> {
            context
                .database
                .get_latest_request(
                    context.selected_profile.as_ref(),
                    recipe_id,
                )
                .map_err(ChainError::Database)
        };
        // Helper to execute the request, if triggered
        let send_request = || async {
            // There are 3 different ways we can generate the request config:
            // 1. Default (enable all query params/headers)
            // 2. Load from UI state for both TUI and CLI
            // 3. Load from UI state for TUI, enable all for CLI
            // These all have their own issues:
            // 1. Triggered request doesn't necessarily match behavior if user
            //  were to execute the request themself
            // 2. CLI behavior is silently controlled by UI state
            // 3. TUI and CLI behavior may not match
            // All 3 options are unintuitive in some way, but 1 is the easiest
            // to implement so I'm going with that for now.
            let build_options = Default::default();

            // Shitty try block
            let result = async {
                let http_engine = context
                    .http_engine
                    .as_ref()
                    .ok_or(TriggeredRequestError::NotAllowed)?;
                let ticket = http_engine
                    .build(
                        RequestSeed::new(recipe.clone(), build_options),
                        context,
                    )
                    .await
                    .map_err(TriggeredRequestError::Build)?;
                ticket
                    .send(&context.database)
                    .await
                    .map_err(TriggeredRequestError::Send)
            };
            result.await.map_err(|error| ChainError::Trigger {
                recipe_id: recipe.id.clone(),
                error,
            })
        };

        // Grab the most recent request in history, or send a new request
        let exchange = match trigger {
            ChainRequestTrigger::Never => {
                get_most_recent()?.ok_or(ChainError::NoResponse)?
            }
            ChainRequestTrigger::NoHistory => {
                // If a exchange is present in history, use that. If not, fetch
                if let Some(exchange) = get_most_recent()? {
                    exchange
                } else {
                    send_request().await?
                }
            }
            ChainRequestTrigger::Expire(duration) => match get_most_recent()? {
                Some(exchange)
                    if exchange.end_time + duration >= Utc::now() =>
                {
                    exchange
                }
                _ => send_request().await?,
            },
            ChainRequestTrigger::Always => send_request().await?,
        };

        // We haven't passed the exchange around so we can unwrap the Arc safely
        Ok(Arc::try_unwrap(exchange.response)
            .expect("Request Arc should have only one reference"))
    }

    /// Extract the specified component bytes from the response.
    /// Returns an error with the missing header if not found.
    fn extract_response_value(
        &self,
        response: ResponseRecord,
        component: &ChainRequestSection,
    ) -> Result<Vec<u8>, ChainError> {
        Ok(match component {
            // This will clone the bytes, which is necessary for the subsequent
            // string conversion anyway
            ChainRequestSection::Body => response.body.into_bytes().into(),
            ChainRequestSection::Header(target_header) => {
                response
                    .headers
                    // If header has multiple values, only grab the first
                    .get(target_header)
                    .ok_or_else(|| ChainError::MissingHeader {
                        header: target_header.clone(),
                    })?
                    .as_bytes()
                    .to_vec()
            }
        })
    }

    /// Render a value from an environment variable
    async fn render_environment_variable(
        &self,
        context: &TemplateContext,
        variable: &Template,
    ) -> Result<Vec<u8>, ChainError> {
        let variable = variable.render_nested("variable", context).await?;
        let value = load_environment_variable(&variable);
        Ok(value.into_bytes())
    }

    /// Render a chained value from a file. Return the files bytes, as well as
    /// its content type if it's known
    async fn render_file(
        &self,
        context: &TemplateContext,
        path: &Template,
    ) -> Result<(Vec<u8>, Option<ContentType>), ChainError> {
        let path: PathBuf = path.render_nested("path", context).await?.into();
        // Guess content type based on file extension
        let content_type = ContentType::from_path(&path).ok();
        let content = fs::read(&path)
            .await
            .map_err(|error| ChainError::File { path, error })?;
        Ok((content, content_type))
    }

    /// Render a chained value from an external command
    async fn render_command(
        &self,
        context: &TemplateContext,
        command: &[Template],
        stdin: Option<&Template>,
    ) -> Result<Vec<u8>, ChainError> {
        // Render each arg in the command
        let command = future::try_join_all(command.iter().enumerate().map(
            |(i, template)| async move {
                template
                    .render_nested(format!("command[{i}]"), context)
                    .await
            },
        ))
        .await?;

        let [program, args @ ..] = command.as_slice() else {
            return Err(ChainError::CommandMissing);
        };

        let _ = debug_span!("Executing command", ?command).entered();

        // Render the stdin template, if present
        let input = if let Some(template) = stdin {
            let input = template.render_nested("stdin", context).await?;

            Some(input)
        } else {
            None
        };

        // Spawn the command process
        let mut process = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| ChainError::Command {
                command: command.to_owned(),
                error,
            })
            .traced()?;

        // Write the stdin to the process
        if let Some(input) = input {
            process
                .stdin
                .as_mut()
                .expect("Process missing stdin")
                .write_all(input.as_bytes())
                .await
                .map_err(|error| ChainError::Command {
                    command: command.to_owned(),
                    error,
                })
                .traced()?;
        }

        // Wait for the process to finish
        let output = process
            .wait_with_output()
            .await
            .map_err(|error| ChainError::Command {
                command: command.to_owned(),
                error,
            })
            .traced()?;

        debug!(
            stdout = %String::from_utf8_lossy(&output.stdout),
            stderr = %String::from_utf8_lossy(&output.stderr),
            "Command success"
        );

        Ok(output.stdout)
    }

    /// Render a value by asking the user to provide it
    async fn render_prompt(
        &self,
        context: &'a TemplateContext,
        message: Option<&Template>,
        default: Option<&Template>,
        sensitive: bool,
    ) -> Result<String, ChainError> {
        // Use the prompter to ask the user a question, and wait for a response
        // on the prompt channel
        let (tx, rx) = oneshot::channel();
        let message = if let Some(template) = message {
            template.render_nested("message", context).await?
        } else {
            self.chain_id.to_string()
        };
        let default = if let Some(template) = default {
            Some(template.render_nested("default", context).await?)
        } else {
            None
        };

        context.prompter.prompt(Prompt {
            message,
            default,
            sensitive,
            channel: tx.into(),
        });
        rx.await.map_err(|_| ChainError::PromptNoResponse)
    }
}

/// A value sourced from the process's environment
struct EnvironmentTemplateSource<'a> {
    variable: &'a str,
}

#[async_trait]
impl<'a> TemplateSource<'a> for EnvironmentTemplateSource<'a> {
    async fn render(&self, _: &'a TemplateContext) -> TemplateResult {
        let value = load_environment_variable(self.variable).into_bytes();
        Ok(RenderedChunk {
            value,
            sensitive: false,
        })
    }
}

impl ChainOutputTrim {
    /// Apply whitespace trimming to string values. If the value is not a valid
    /// string, no trimming is applied
    fn apply(self, value: Vec<u8>) -> Vec<u8> {
        // Theoretically we could strip whitespace-looking characters from
        // binary data, but if the whole thing isn't a valid string it doesn't
        // really make any sense to.
        let Ok(s) = std::str::from_utf8(&value) else {
            return value;
        };
        match self {
            Self::None => value,
            Self::Start => s.trim_start().into(),
            Self::End => s.trim_end().into(),
            Self::Both => s.trim().into(),
        }
    }
}

/// Load variable from environment. If the variable is missing or otherwise
/// inaccessible, return an empty string. This models standard shell behavior,
/// so it should be intuitive for users.
///
/// The variable will be loaded as a **string**, not bytes. This is because the
/// raw byte representation varies by OS. We're choosing a uniform experience
/// over the ability to load non-string bytes from an env variable, because
/// that's an extremely niche use case.
fn load_environment_variable(variable: &str) -> String {
    env::var(variable).unwrap_or_default()
}
