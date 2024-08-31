//! Template rendering implementation

use crate::{
    collection::{
        ChainId, ChainOutputTrim, ChainRequestSection, ChainRequestTrigger,
        ChainSource, RecipeId,
    },
    http::{content_type::ContentType, Exchange, RequestSeed, ResponseRecord},
    template::{
        error::TriggeredRequestError, parse::TemplateInputChunk, ChainError,
        Prompt, Select, Template, TemplateChunk, TemplateContext,
        TemplateError, TemplateKey,
    },
    util::{expand_home, FutureCache, FutureCacheOutcome, ResultTraced},
};
use async_trait::async_trait;
use chrono::Utc;
use futures::future;
use std::{env, path::PathBuf, process::Stdio, sync::Arc};
use tokio::{fs, io::AsyncWriteExt, process::Command, sync::oneshot};
use tracing::{debug, debug_span, error, instrument, trace, trace_span};

/// Outcome of rendering a single chunk. This allows attaching some metadata to
/// the render.
#[derive(Clone, Debug)]
struct RenderedChunk {
    /// This is wrapped in `Arc` to de-duplicate large values derived from
    /// chains. When the same chain is used multiple times in a render group it
    /// gets deduplicated, meaning multiple render results would refer to the
    /// same data. In the vast majority of cases though we only ever have one
    /// pointer to this data.
    value: Arc<Vec<u8>>,
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
        self.render_impl(context, &mut RenderKeyStack::default())
            .await
    }

    /// Render the template using values from the given context. If any chunk
    /// failed to render, return an error. The rendered template will be
    /// converted from raw bytes to UTF-8. If it is not valid UTF-8, return an
    /// error.
    pub async fn render_string(
        &self,
        context: &TemplateContext,
    ) -> Result<String, TemplateError> {
        self.render_string_impl(context, &mut RenderKeyStack::default())
            .await
    }

    /// Render the template string using values from the given context,
    /// returning the individual rendered chunks. This is useful in any
    /// application where rendered chunks need to be handled differently from
    /// raw chunks, e.g. in render previews.
    pub async fn render_chunks(
        &self,
        context: &TemplateContext,
    ) -> Vec<TemplateChunk> {
        self.render_chunks_impl(context, &mut RenderKeyStack::default())
            .await
    }

    /// Internal version of [Self::render] with local render state
    async fn render_impl<'a>(
        &'a self,
        context: &'a TemplateContext,
        stack: &mut RenderKeyStack<'a>,
    ) -> Result<Vec<u8>, TemplateError> {
        // Render each individual template chunk in the string
        let chunks = self.render_chunks_impl(context, stack).await;

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
                TemplateChunk::Rendered { value, .. } => {
                    // Only clone if we have multiple copies of this data, which
                    // only occurs if a chain is used more than once
                    buf.extend(Arc::unwrap_or_clone(value))
                }
                TemplateChunk::Error(error) => return Err(error),
            }
        }

        Ok(buf)
    }

    /// Internal version of [Self::render_string] with local render state
    async fn render_string_impl<'a>(
        &'a self,
        context: &'a TemplateContext,
        stack: &mut RenderKeyStack<'a>,
    ) -> Result<String, TemplateError> {
        let bytes = self.render_impl(context, stack).await?;
        String::from_utf8(bytes).map_err(TemplateError::InvalidUtf8)
    }

    /// Internal version of [Self::render_chunks] with local render state
    #[instrument(skip_all, fields(template = %self.display()))]
    async fn render_chunks_impl<'a>(
        &'a self,
        context: &'a TemplateContext,
        stack: &mut RenderKeyStack<'a>,
    ) -> Vec<TemplateChunk> {
        async fn render_key<'a>(
            key: &'a TemplateKey,
            context: &'a TemplateContext,
            stack: &mut RenderKeyStack<'a>,
        ) -> TemplateResult {
            // The formatted key should match the source that it was parsed
            // from, therefore we can use it to match the override key
            let raw = key.to_string();

            // If the key is in the overrides, use the given value
            // without parsing it
            match context.overrides.get(&raw) {
                Some(value) => {
                    trace!(
                        key = raw,
                        value,
                        "Rendered template key from override"
                    );
                    Ok(RenderedChunk {
                        value: value.clone().into_bytes().into(),
                        // The overriden value *could* be marked
                        // sensitive, but we're taking a shortcut and
                        // assuming it isn't
                        sensitive: false,
                    })
                }
                None => {
                    let span = trace_span!("Rendering template key", key = raw);
                    let _ = span.enter();
                    stack.push(key)?;
                    // Standard case - parse the key and render it
                    let result = key.to_source().render(context, stack).await;
                    stack.pop();
                    if let Ok(value) = &result {
                        trace!(?value, "Rendered template key to value");
                    }
                    result
                }
            }
        }

        // Map over each parsed chunk, and render the keys into strings. The
        // raw text chunks will be mapped 1:1. This clone is pretty cheap
        // because raw text uses Arc and keys just contain metadata
        let futures = self.chunks.iter().map(|chunk| {
            // Fork the local state, one copy for each new branch we're spawning
            let mut stack = stack.clone();
            async move {
                match chunk {
                    TemplateInputChunk::Raw(text) => {
                        TemplateChunk::Raw(Arc::clone(text))
                    }
                    TemplateInputChunk::Key(key) => {
                        render_key(key, context, &mut stack).await.into()
                    }
                }
            }
        });

        // Parallelization!
        future::join_all(futures).await
    }

    /// Render a template whose result will be used as configuration for a
    /// chain. It's assumed we need string output for that. The given field name
    /// will be used to provide a descriptive error.
    async fn render_chain_config<'a>(
        &'a self,
        field: impl Into<String>,
        context: &'a TemplateContext,
        stack: &mut RenderKeyStack<'a>,
    ) -> Result<String, ChainError> {
        self.render_string_impl(context, stack)
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
    async fn render(
        &self,
        context: &'a TemplateContext,
        stack: &mut RenderKeyStack<'a>,
    ) -> TemplateResult;
}

/// A simple field value (e.g. from the profile or an override)
struct FieldTemplateSource<'a> {
    field: &'a str,
}

#[async_trait]
impl<'a> TemplateSource<'a> for FieldTemplateSource<'a> {
    async fn render(
        &self,
        context: &'a TemplateContext,
        stack: &mut RenderKeyStack<'a>,
    ) -> TemplateResult {
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
        let rendered =
            template
                .render_impl(context, stack)
                .await
                .map_err(|error| TemplateError::FieldNested {
                    field: field.to_owned(),
                    error: Box::new(error),
                })?;
        Ok(RenderedChunk {
            value: rendered.into(),
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
    async fn render(
        &self,
        context: &'a TemplateContext,
        stack: &mut RenderKeyStack<'a>,
    ) -> TemplateResult {
        // Check the chain cache to see if this value is already being computed
        // somewhere else. If it is, we'll block on that and re-use the result.
        // If not, we get a guard back, meaning we're responsible for the
        // computation. At the end, we'll write back to the guard so everyone
        // else can copy our homework.
        let cache = &context.state.chain_results;
        let guard = match cache.get_or_init(self.chain_id.clone()).await {
            FutureCacheOutcome::Hit(result) => return result,
            FutureCacheOutcome::Miss(guard) => guard,
            // The future responsible for writing to the guard didn't. That's a
            // very unlikely logic bug so worth a panic
            FutureCacheOutcome::NoResponse => {
                panic!("Cached future did not set a value. This is a bug!")
            }
        };

        // Any error in here is the chain error subtype
        let result: TemplateResult = async {
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
                    self.render_command(
                        context,
                        stack,
                        command,
                        stdin.as_ref(),
                    )
                    .await?,
                    // No way to guess content type on this
                    None,
                ),
                ChainSource::File { path } => {
                    self.render_file(context, stack, path).await?
                }
                ChainSource::Environment { variable } => (
                    self.render_environment_variable(context, stack, variable)
                        .await?,
                    // No way to guess content type on this
                    None,
                ),
                ChainSource::Prompt { message, default } => (
                    self.render_prompt(
                        context,
                        stack,
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
                        ContentType::from_headers(&response.headers).ok();
                    let value = self
                        .extract_response_value(
                            context, stack, response, section,
                        )
                        .await?;
                    (value, content_type)
                }
                ChainSource::Select { message, options } => (
                    self.render_select(
                        context,
                        stack,
                        message.as_ref(),
                        options,
                    )
                    .await?
                    .into_bytes(),
                    None,
                ),
            };
            // If the user provided a content type, prefer that over the
            // detected one
            let content_type = chain.content_type.or(content_type);

            // If a selector path is present, filter down the value
            let value = if let Some(selector) = &chain.selector {
                let content_type =
                    content_type.ok_or(ChainError::UnknownContentType)?;
                // Parse according to detected content type
                let value =
                    content_type.parse_content(&value).map_err(|error| {
                        ChainError::ParseResponse {
                            error: error.into(),
                        }
                    })?;
                selector.query_to_string(&*value)?.into_bytes()
            } else {
                value
            };

            Ok(RenderedChunk {
                value: chain.trim.apply(value).into(),
                sensitive: chain.sensitive,
            })
        }
        .await
        .map_err(|error| TemplateError::Chain {
            chain_id: self.chain_id.clone(),
            error,
        });

        // Store value in the cache so other instances of this chain can use it
        guard.set(result.clone());

        result
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
                .map_err(|error| ChainError::Database(error.into()))
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
                        RequestSeed::new(recipe_id.clone(), build_options),
                        context,
                    )
                    .await
                    .map_err(|error| {
                        TriggeredRequestError::Build(error.into())
                    })?;
                ticket
                    .send(&context.database)
                    .await
                    .map_err(|error| TriggeredRequestError::Send(error.into()))
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

    /// Extract the specified component bytes from the response. For headers,
    /// the header name is a template so we'll render that.
    async fn extract_response_value(
        &self,
        context: &'a TemplateContext,
        stack: &mut RenderKeyStack<'a>,
        response: ResponseRecord,
        component: &'a ChainRequestSection,
    ) -> Result<Vec<u8>, ChainError> {
        Ok(match component {
            // This will clone the bytes, which is necessary for the subsequent
            // string conversion anyway
            ChainRequestSection::Body => response.body.into_bytes().into(),
            ChainRequestSection::Header(header) => {
                let header = header
                    .render_chain_config("section", context, stack)
                    .await?;
                response
                    .headers
                    // If header has multiple values, only grab the first
                    .get(&header)
                    .ok_or_else(|| ChainError::MissingHeader { header })?
                    .as_bytes()
                    .to_vec()
            }
        })
    }

    /// Render a value from an environment variable
    async fn render_environment_variable(
        &self,
        context: &'a TemplateContext,
        stack: &mut RenderKeyStack<'a>,
        variable: &'a Template,
    ) -> Result<Vec<u8>, ChainError> {
        let variable = variable
            .render_chain_config("variable", context, stack)
            .await?;
        let value = load_environment_variable(&variable);
        Ok(value.into_bytes())
    }

    /// Render a chained value from a file. Return the files bytes, as well as
    /// its content type if it's known
    async fn render_file(
        &self,
        context: &'a TemplateContext,
        stack: &mut RenderKeyStack<'a>,
        path: &'a Template,
    ) -> Result<(Vec<u8>, Option<ContentType>), ChainError> {
        let path: PathBuf = path
            .render_chain_config("path", context, stack)
            .await?
            .into();
        let path = expand_home(path).into_owned(); // Expand ~

        // Guess content type based on file extension
        let content_type = ContentType::from_path(&path).ok();
        let content =
            fs::read(&path).await.map_err(|error| ChainError::File {
                path,
                error: error.into(),
            })?;
        Ok((content, content_type))
    }

    /// Render a chained value from an external command
    async fn render_command(
        &self,
        context: &'a TemplateContext,
        stack: &mut RenderKeyStack<'a>,
        command: &'a [Template],
        stdin: Option<&'a Template>,
    ) -> Result<Vec<u8>, ChainError> {
        // Render each arg in the command
        let command = future::try_join_all(command.iter().enumerate().map(
            |(i, template)| {
                // Fork the local state, one copy for each new branch
                let mut stack = stack.clone();
                async move {
                    template
                        .render_chain_config(
                            format!("command[{i}]"),
                            context,
                            &mut stack,
                        )
                        .await
                }
            },
        ))
        .await?;

        let [program, args @ ..] = command.as_slice() else {
            return Err(ChainError::CommandMissing);
        };

        let _ = debug_span!("Executing command", ?command).entered();

        // Render the stdin template, if present
        let input = if let Some(template) = stdin {
            let input = template
                .render_chain_config("stdin", context, stack)
                .await?;

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
                error: error.into(),
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
                    error: error.into(),
                })
                .traced()?;
        }

        // Wait for the process to finish
        let output = process
            .wait_with_output()
            .await
            .map_err(|error| ChainError::Command {
                command: command.to_owned(),
                error: error.into(),
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
        stack: &mut RenderKeyStack<'a>,
        message: Option<&'a Template>,
        default: Option<&'a Template>,
        sensitive: bool,
    ) -> Result<String, ChainError> {
        // Use the prompter to ask the user a question, and wait for a response
        // on the prompt channel
        let (tx, rx) = oneshot::channel();
        let message = if let Some(template) = message {
            template
                .render_chain_config("message", context, stack)
                .await?
        } else {
            self.chain_id.to_string()
        };
        let default = if let Some(template) = default {
            Some(
                template
                    .render_chain_config("default", context, stack)
                    .await?,
            )
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

    async fn render_select(
        &self,
        context: &'a TemplateContext,
        stack: &mut RenderKeyStack<'a>,
        message: Option<&'a Template>,
        options: &'a [Template],
    ) -> Result<String, ChainError> {
        let (tx, rx) = oneshot::channel();
        let message = if let Some(template) = message {
            template
                .render_chain_config("message", context, stack)
                .await?
        } else {
            self.chain_id.to_string()
        };

        let options = future::try_join_all(options.iter().enumerate().map(
            |(i, template)| {
                // Fork the local state, one copy for each new branch
                let mut stack = stack.clone();
                async move {
                    template
                        .render_chain_config(
                            format!("options[{i}]"),
                            context,
                            &mut stack,
                        )
                        .await
                }
            },
        ))
        .await?;

        context.prompter.select(Select {
            message,
            options,
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
    async fn render(
        &self,
        _: &'a TemplateContext,
        _: &mut RenderKeyStack,
    ) -> TemplateResult {
        let value = load_environment_variable(self.variable).into_bytes();
        Ok(RenderedChunk {
            value: value.into(),
            sensitive: false,
        })
    }
}

/// State for a render group, which consists of one or more related renders
/// (e.g. all the template renders for a single recipe). This state is stored in
/// the template context.
#[derive(Debug, Default)]
pub struct RenderGroupState {
    /// Cache the result of each chain, so multiple references to the same
    /// chain within a render group don't have to do the work multiple
    /// times.
    chain_results: FutureCache<ChainId, TemplateResult>,
}

/// Track the series of template keys that we've followed to get to the current
/// spot in the render. This is used to detect cycles in templates, to prevent
/// infinite loops. This tracks a **single branch** of a single template's
/// render tree. Each time a nested template key is encountered, we trigger a
/// nested render and push onto the stack. If multiple nested keys are found,
/// state is forked to maintain a stack for each branch separately.
#[derive(Clone, Debug, Default)]
struct RenderKeyStack<'a>(Vec<&'a TemplateKey>);

impl<'a> RenderKeyStack<'a> {
    /// Push an additional key onto the render stack. If the key is already in
    /// the stack, that indicates a cycle and we'll return an error. This should
    /// be called *before* rendering the given key, and popped immediately after
    /// rendering it.
    fn push(
        &mut self,
        template_key: &'a TemplateKey,
    ) -> Result<(), TemplateError> {
        if self.0.contains(&template_key) {
            // Push anyway so we show the full cycle in the error
            self.0.push(template_key);
            Err(TemplateError::InfiniteLoop(
                self.0.iter().copied().cloned().collect(),
            ))
        } else {
            self.0.push(template_key);
            Ok(())
        }
    }

    /// Pop the last key off the render stack. Call immediately after a key
    /// finishes rendering.
    fn pop(&mut self) {
        if self.0.pop().is_none() {
            // Indicates some sort of logic bug
            error!("Pop attempted on empty template key stack");
        }
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
