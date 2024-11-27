//! Template rendering implementation

use crate::{
    lua::LuaRenderer,
    template::{
        parse::TemplateInputChunk, Template, TemplateChunk, TemplateError,
        TemplateExpression,
    },
};
use futures::future;
use std::sync::Arc;
use tracing::{instrument, trace, trace_span};

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
        renderer: &LuaRenderer,
    ) -> Result<Vec<u8>, TemplateError> {
        // Render each individual template chunk in the string
        let chunks = self.render_chunks(renderer).await;

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

    /// Render the template using values from the given context. If any chunk
    /// failed to render, return an error. The rendered template will be
    /// converted from raw bytes to UTF-8. If it is not valid UTF-8, return an
    /// error.
    pub async fn render_string(
        &self,
        renderer: &LuaRenderer,
    ) -> Result<String, TemplateError> {
        let bytes = self.render(renderer).await?;
        let s = String::from_utf8(bytes)?;
        Ok(s)
    }

    /// Render the template string using values from the given context,
    /// returning the individual rendered chunks. This is useful in any
    /// application where rendered chunks need to be handled differently from
    /// raw chunks, e.g. in render previews.
    #[instrument(level = "debug", skip_all, fields(template = %self.display()))]
    pub async fn render_chunks(
        &self,
        renderer: &LuaRenderer,
    ) -> Vec<TemplateChunk> {
        async fn render_key<'a>(
            key: &'a TemplateExpression,
            renderer: &'a LuaRenderer,
        ) -> TemplateResult {
            // The formatted key should match the source that it was parsed
            // from, therefore we can use it to match the override key
            let raw = key.to_string();

            // If the key is in the overrides, use the given value
            // without parsing it
            match renderer.context().overrides.get(&raw) {
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
                    let _ = trace_span!("Rendering template key", key = raw)
                        .enter();
                    // Standard case - parse the key and render it
                    let result = key.render(renderer).await;
                    if let Ok(value) = &result {
                        trace!(?value, "Rendered template key to value");
                    }
                    result
                }
            }
        }

        // Map over each parsed chunk, and render the keys into strings. The
        // raw text chunks will be mapped 1:1. This clone is pretty cheap
        // because raw text uses Arc and expressions tend to be small
        let futures = self.chunks.iter().map(|chunk| async move {
            match chunk {
                TemplateInputChunk::Raw(text) => {
                    TemplateChunk::Raw(Arc::clone(text))
                }
                TemplateInputChunk::Expression(key) => {
                    render_key(key, renderer).await.into()
                }
            }
        });

        // Parallelization!
        future::join_all(futures).await
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

impl TemplateExpression {
    async fn render(&self, context: &LuaRenderer) -> TemplateResult {
        let rendered = context.eval(&self.source).await?;
        Ok(RenderedChunk {
            value: rendered.into(),
            sensitive: false, // TODO support somehow
        })
    }
}
