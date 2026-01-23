//! Generate strings (and bytes) from user-written templates with dynamic data.
//! This engine is focused on rendering templates, and is generally agnostic of
//! its usage in the rest of the app. As such, there is no logic in here
//! relating to HTTP or other Slumber concepts.

mod cereal;
mod display;
mod error;
mod expression;
mod parse;
#[cfg(test)]
mod test_util;
#[cfg(test)]
mod tests;
mod value;

pub use error::{
    Expected, RenderError, TemplateParseError, ValueError, WithValue,
};
pub use expression::{Expression, FunctionCall, Identifier, Literal};
pub use value::{
    Arguments, FunctionOutput, LazyValue, StreamSource, TryFromValue, Value,
};

use bytes::{Bytes, BytesMut};
use futures::{Stream, StreamExt, TryStreamExt, future, stream};
use itertools::Itertools;
#[cfg(test)]
use proptest::{arbitrary::any, strategy::Strategy};
use slumber_util::NEW_ISSUE_LINK;
use std::{fmt::Debug, sync::Arc};

/// `Context` defines how template fields and functions are resolved. Both
/// field resolution and function calls can be asynchronous.
pub trait Context: Sized {
    /// Does the render target support streaming? Typically this should return
    /// `false`.
    ///
    /// This is a method on the context to avoid plumbing around a second object
    /// to all render locations.
    fn can_stream(&self) -> bool;

    /// Get the value of a field from the context. The implementor can decide
    /// where fields are derived from. Fields can also be computed dynamically
    /// and be `async`. For example, fields can be loaded from a map of nested
    /// templates, in which case the nested template would need to be rendered
    /// before this can be returned.
    async fn get_field(
        &self,
        identifier: &Identifier,
    ) -> Result<LazyValue, RenderError>;

    /// Call a function by name
    async fn call(
        &self,
        function_name: &Identifier,
        arguments: Arguments<'_, Self>,
    ) -> Result<LazyValue, RenderError>;
}

/// A parsed template, which can contain raw and/or templated content. The
/// string is parsed during creation to identify template keys, hence the
/// immutability.
///
/// The original string is *not* stored. To recover the source string, use the
/// `Display` implementation.
///
/// Invariants:
/// - Two templates with the same source string will have the same set of
///   chunks, and vice versa
/// - No two raw chunks will ever be consecutive
/// - Raw chunks cannot not be empty
#[derive(Clone, Debug, Default, PartialEq)]
#[cfg_attr(test, derive(proptest_derive::Arbitrary))]
pub struct Template {
    /// Pre-parsed chunks of the template. For raw chunks we store the
    /// presentation text (which is not necessarily the source text, as escape
    /// sequences will be eliminated). For keys, just store the needed
    /// metadata.
    #[cfg_attr(
        test,
        proptest(
            strategy = "any::<Vec<TemplateChunk>>().prop_map(test_util::join_raw)"
        )
    )]
    chunks: Vec<TemplateChunk>,
}

impl Template {
    /// Compile a template from its composite chunks
    ///
    /// ## Panics
    ///
    /// Panic if the chunk list is invalid:
    ///
    /// - If there are consecutive raw chunks
    /// - If any raw chunk is empty
    ///
    /// These panics are necessary to maintain the invariants documented on the
    /// struct definition.
    pub fn from_chunks(chunks: Vec<TemplateChunk>) -> Self {
        // Since the chunks are constructed externally, we need to enforce our
        // invariants. This will short-circuit any bugs in chunk construction
        assert!(
            // Look for empty raw chunks
            !chunks.iter().any(
                |chunk| matches!(chunk, TemplateChunk::Raw(s) if s.is_empty())
            )
            // Look for consecutive raw chunks
            && !chunks.iter().tuple_windows().any(|pair| matches!(
                pair,
                (TemplateChunk::Raw(_), TemplateChunk::Raw(_))
            )),
            "Invalid chunks in generated template {chunks:?} This is a bug! \
            Please report it. {NEW_ISSUE_LINK}"
        );
        Self { chunks }
    }

    /// Create a new template from a raw string, without parsing it at all.
    /// Useful when importing from external formats where the string isn't
    /// expected to be a valid Slumber template
    pub fn raw(template: String) -> Template {
        let chunks = if template.is_empty() {
            vec![]
        } else {
            // This may seem too easy, but the hard part comes during
            // stringification, when we need to add backslashes to get the
            // string to parse correctly later
            vec![TemplateChunk::Raw(template.into())]
        };
        Self { chunks }
    }

    /// Create a template that loads a file
    ///
    /// ```
    /// use slumber_template::Template;
    ///
    /// let template = Template::file("path/to/file".into());
    /// assert_eq!(template.display(), "{{ file('path/to/file') }}");
    /// ```
    pub fn file(path: String) -> Template {
        Self::function_call("file", [path.into()], [])
    }

    /// Create a new template that contains a single chunk, which is an
    /// expression that invokes a function with position arguments and optional
    /// keyword arguments.
    ///
    /// ```
    /// # use slumber_template::Template;
    /// let template = Template::function_call(
    ///     "hello",
    ///     ["john".into()],
    ///     [("mode", Some("caps".into()))],
    /// );
    /// assert_eq!(template.display(), "{{ hello('john', mode='caps') }}");
    /// ```
    pub fn function_call(
        name: &'static str,
        position: impl IntoIterator<Item = Expression>,
        keyword: impl IntoIterator<Item = (&'static str, Option<Expression>)>,
    ) -> Self {
        let chunks = vec![TemplateChunk::Expression(Expression::call(
            name, position, keyword,
        ))];
        Self { chunks }
    }

    /// Is the template an empty string?
    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }

    /// Does the template have at least one dynamic chunk? If this returns
    /// `false`, the template will always render to its source text
    pub fn is_dynamic(&self) -> bool {
        self.chunks
            .iter()
            .any(|chunk| matches!(chunk, TemplateChunk::Expression(_)))
    }

    /// Render the template, returning the individual rendered chunks rather
    /// than stitching them together into a string. If any individual chunk
    /// fails to render, its error will be returned inline as
    /// [RenderedChunk::Error] and the rest of the template will still be
    /// rendered. The returned output can be transformed into a variety of final
    /// output types.
    pub async fn render<Ctx: Context>(&self, context: &Ctx) -> RenderedOutput {
        // Map over each parsed chunk, and render the expressions into values.
        // because raw text uses Arc and expressions just contain metadata
        // The raw text chunks will be mapped 1:1. This clone is pretty cheap
        let futures = self.chunks.iter().map(|chunk| async move {
            match chunk {
                TemplateChunk::Raw(text) => {
                    RenderedChunk::Raw(Arc::clone(text))
                }
                TemplateChunk::Expression(expression) => {
                    match expression.render(context).await {
                        Ok(lazy) if context.can_stream() => {
                            RenderedChunk::Rendered(lazy)
                        }
                        // If the context doesn't support streaming, resolve
                        // the lazy value now
                        Ok(lazy) => lazy
                            .resolve()
                            .await
                            .map_or_else(RenderedChunk::Error, |value| {
                                RenderedChunk::Rendered(value.into())
                            }),
                        Err(error) => RenderedChunk::Error(error),
                    }
                }
            }
        });

        // Concurrency!
        let chunks = future::join_all(futures).await;
        RenderedOutput(chunks)
    }

    /// Convenience method for rendering a template and collecting the output
    /// into a byte string.
    pub async fn render_bytes<Ctx: Context>(
        &self,
        context: &Ctx,
    ) -> Result<Bytes, RenderError> {
        self.render(context).await.try_collect_bytes().await
    }

    /// Convenience method for rendering a template and collecting the output
    /// into a string. If the output is not valid UTF-8, return an error.
    pub async fn render_string<Ctx: Context>(
        &self,
        context: &Ctx,
    ) -> Result<String, RenderError> {
        let bytes = self.render_bytes(context).await?;
        String::from_utf8(bytes.into()).map_err(RenderError::other)
    }
}

/// Build a single-chunk template
impl From<Expression> for Template {
    fn from(expression: Expression) -> Self {
        Self::from_chunks(vec![TemplateChunk::Expression(expression)])
    }
}

/// Parse template from a string literal. Panic if invalid
#[cfg(any(test, feature = "test"))]
impl From<&'static str> for Template {
    fn from(value: &'static str) -> Self {
        value.parse().unwrap()
    }
}

#[cfg(any(test, feature = "test"))]
impl<const N: usize> From<[TemplateChunk; N]> for Template {
    fn from(chunks: [TemplateChunk; N]) -> Self {
        Self {
            chunks: chunks.into(),
        }
    }
}

/// A parsed piece of a template. After parsing, each chunk is either raw text
/// or a parsed key, ready to be rendered.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(test, derive(proptest_derive::Arbitrary))]
pub enum TemplateChunk {
    /// Raw unprocessed text, i.e. something **outside** the `{{ }}`. This is
    /// stored in an `Arc` so we can share cheaply in each render without
    /// having to clone text. This works because templates are immutable. Any
    /// non-empty string is a valid raw chunk. This text represents what the
    /// user wants to see, i.e. it does *not* including any escape chars.
    Raw(
        #[cfg_attr(test, proptest(strategy = "\".+\".prop_map(String::into)"))]
        Arc<str>,
    ),
    /// Dynamic expression to be computed at render time
    Expression(
        #[cfg_attr(
            test,
            proptest(strategy = "test_util::expression_arbitrary()")
        )]
        Expression,
    ),
}

#[cfg(test)]
impl From<Expression> for TemplateChunk {
    fn from(expression: Expression) -> Self {
        Self::Expression(expression)
    }
}

/// Outcome of rendering the individual chunks of a template. This is an
/// intermediate output type that can be resolved into a variety of final
/// output types.
#[derive(Debug)]
pub struct RenderedOutput(Vec<RenderedChunk>);

impl RenderedOutput {
    /// If this output is a single chunk and that chunk is a stream, get the
    /// source of the stream
    pub fn stream_source(&self) -> Option<&StreamSource> {
        if let [RenderedChunk::Rendered(LazyValue::Stream { source, .. })] =
            self.0.as_slice()
        {
            Some(source)
        } else {
            None
        }
    }

    /// Does this output contain *any* stream chunks?
    pub fn has_stream(&self) -> bool {
        self.0.iter().any(|chunk| match chunk {
            RenderedChunk::Raw(_) => false,
            RenderedChunk::Rendered(LazyValue::Value(_)) => false,
            RenderedChunk::Rendered(LazyValue::Stream { .. }) => true,
            // Recursion!!
            RenderedChunk::Rendered(LazyValue::Nested(output)) => {
                output.has_stream()
            }
            RenderedChunk::Error(_) => true,
        })
    }

    /// Unpack this output into a single lazy value. If the output is a single
    /// dynamic chunk, unpack it into a scalar value. Otherwise return a
    /// [LazyValue::Nested].
    pub fn unpack(mut self) -> LazyValue {
        if let &[RenderedChunk::Rendered(_)] = self.0.as_slice() {
            // If we have a single dynamic chunk, return its value directly
            let Some(RenderedChunk::Rendered(lazy)) = self.0.pop() else {
                // Checked pattern above
                unreachable!()
            };
            lazy
        } else {
            LazyValue::Nested(self)
        }
    }

    /// Convert this output into a byte stream. Each chunk will be yielded as a
    /// separate `Bytes` output from the stream, except for inner stream chunks,
    /// which can yield any number of values based on their
    /// implementation. Return `Err` if any of the rendered chunks are errors.
    pub fn try_into_stream(
        self,
    ) -> Result<
        impl Stream<Item = Result<Bytes, RenderError>> + Send,
        RenderError,
    > {
        let stream_value =
            |bytes| Ok(stream::once(future::ready(Ok(bytes))).boxed());

        // First, make sure we have no errors
        let chunks = self
            .0
            .into_iter()
            .map(move |chunk| match chunk {
                RenderedChunk::Raw(s) => {
                    stream_value(Bytes::from(s.to_string()))
                }
                RenderedChunk::Rendered(lazy) => match lazy {
                    LazyValue::Value(value) => stream_value(value.into_bytes()),
                    LazyValue::Stream { stream, .. } => Ok(stream.boxed()),
                    LazyValue::Nested(output) => {
                        Ok(output.try_into_stream()?.boxed())
                    }
                },

                RenderedChunk::Error(error) => Err(error),
            })
            .collect::<Result<Vec<_>, _>>()?;

        // If none of the chunks failed, we can chain all the streams together
        Ok(stream::iter(chunks).flatten())
    }

    /// Collect the rendered chunks into a [Value] by these rules:
    /// - If the template is a single dynamic chunk, return the output of that
    ///   chunk, which may be any type of [Value]
    /// - Any other template will be rendered to a string by stringifying each
    ///   dynamic chunk and concatenating them all together
    /// - If rendering to a string fails because the bytes are not valid UTF-8,
    ///   concatenate into a bytes object instead
    pub async fn try_collect_value(self) -> Result<Value, RenderError> {
        // If we only have one chunk, unpack it into a value
        let value = match self.unpack() {
            LazyValue::Value(value) => value,
            lazy @ LazyValue::Stream { .. } => lazy.resolve().await?,
            LazyValue::Nested(output) => {
                // Render to bytes
                let bytes = output.try_collect_bytes().await?;
                Value::Bytes(bytes)
            }
        };

        // Try to convert bytes to string, because that's generally more
        // useful to the consumer
        match value {
            Value::Bytes(bytes) => match String::from_utf8(bytes.into()) {
                Ok(s) => Ok(Value::String(s)),
                Err(error) => Ok(Value::Bytes(error.into_bytes().into())),
            },
            _ => Ok(value),
        }
    }

    /// Collect the rendered chunks into a byte string. If any chunk is an
    /// error, return an error. This is async because the chunk may be a
    /// stream, in which case it will be resolved.
    pub async fn try_collect_bytes(self) -> Result<Bytes, RenderError> {
        // Build a stream, then collect it into bytes
        self.try_into_stream()?
            .try_collect::<BytesMut>()
            .await
            .map(Bytes::from)
    }
}

/// Get an iterator over the chunks of this output
impl IntoIterator for RenderedOutput {
    type Item = RenderedChunk;
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

/// A piece of a rendered template string. A collection of chunks collectively
/// constitutes a rendered string when displayed contiguously.
#[derive(Debug)]
pub enum RenderedChunk {
    /// Raw unprocessed text, i.e. something **outside** the `{{ }}`. This is
    /// stored in an `Arc` so we can reference the text in the parsed input
    /// without having to clone it.
    Raw(Arc<str>),
    /// A dynamic chunk of a template, rendered to a stream/value
    Rendered(LazyValue),
    /// An error occurred while rendering a template key
    Error(RenderError),
}

#[cfg(test)]
impl PartialEq for RenderedChunk {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Raw(raw1), Self::Raw(raw2)) => raw1 == raw2,
            (
                Self::Rendered(LazyValue::Value(value1)),
                Self::Rendered(LazyValue::Value(value2)),
            ) => value1 == value2,
            // Streams are never equal
            (Self::Rendered(_), Self::Rendered(_)) => false,
            (Self::Error(error1), Self::Error(error2)) => {
                // RenderError doesn't have a PartialEq impl, so we have to
                // do a string comparison.
                error1.to_string() == error2.to_string()
            }
            _ => false,
        }
    }
}
