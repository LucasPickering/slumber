//! Template rendering implementation

use crate::{
    collection::ChainOutputTrim,
    template::{
        ChainError, Template, TemplateChunk, TemplateContext, TemplateError,
    },
    util::FutureCache,
};
use futures::future;
use std::{env, sync::Arc};
use tracing::{error, instrument, trace, trace_span};

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
    #[instrument(level = "debug", skip_all, fields(template = %self.display()))]
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
                        // The overridden value *could* be marked
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
