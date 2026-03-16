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
    Arguments, FunctionOutput, StreamSource, TryFromValue, Value, ValueStream,
};

use crate::{parse::MODIFIER_UNPACK, value::RenderValue};
use bytes::{Bytes, BytesMut};
use futures::{Stream, StreamExt, TryStreamExt, future, stream};
use itertools::Itertools;
#[cfg(test)]
use proptest::{arbitrary::any, strategy::Strategy};
use slumber_util::NEW_ISSUE_LINK;
use std::{
    fmt::{self, Debug, Display},
    slice,
    sync::Arc,
};

/// `Context` defines how template fields and functions are resolved. Both
/// field resolution and function calls can be asynchronous.
///
///
/// `V` is the type of value rendered with this context. Generally [Value] or
/// [ValueStream].
pub trait Context<V>: Sized {
    /// Get the value of a field from the context. The implementor can decide
    /// where fields are derived from. Fields can also be computed dynamically
    /// and be `async`. For example, fields can be loaded from a map of nested
    /// templates, in which case the nested template would need to be rendered
    /// before this can be returned.
    async fn get_field(
        &self,
        identifier: &Identifier,
    ) -> Result<V, RenderError>;

    /// Call a function by name
    async fn call(
        &self,
        function_name: &Identifier,
        arguments: Arguments<'_, Self>,
    ) -> Result<V, RenderError>;
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
    pub fn raw(template: String) -> Self {
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
        let chunks = vec![TemplateChunk::Expression {
            expression: Expression::call(name, position, keyword),
            modifier: None,
        }];
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
            .any(|chunk| matches!(chunk, TemplateChunk::Expression { .. }))
    }

    /// Render the template, returning the individual rendered chunks rather
    /// than stitching them together into a string
    ///
    /// If any individual chunk fails to render, its error will be returned
    /// inline as [RenderedChunk::Error] and the rest of the template will still
    /// be rendered. The returned output can be transformed into a variety of
    /// final output types.
    ///
    /// Use this for cases that do *not* support streaming.
    pub async fn render_chunks<Ctx>(
        &self,
        context: &Ctx,
    ) -> RenderedChunks<Value>
    where
        Ctx: Context<Value>,
    {
        self.render_chunks_inner(context).await
    }

    /// Render the template with streaming supported
    ///
    /// This is [Self::render_chunks] but streams are *not* resolved eagerly.
    /// Use this for cases where streams can be used natively.
    pub async fn render_chunks_stream<Ctx>(
        &self,
        context: &Ctx,
    ) -> RenderedChunks<ValueStream>
    where
        Ctx: Context<ValueStream>,
    {
        self.render_chunks_inner(context).await
    }

    /// Convenience method for rendering a template and collecting the output
    /// into a byte string.
    pub async fn render_bytes<Ctx: Context<Value>>(
        &self,
        context: &Ctx,
    ) -> Result<Bytes, RenderError> {
        self.render_chunks(context).await.try_into_bytes()
    }

    /// Convenience method for rendering a template and collecting the output
    /// into a string. If the output is not valid UTF-8, return an error.
    pub async fn render_string<Ctx: Context<Value>>(
        &self,
        context: &Ctx,
    ) -> Result<String, RenderError> {
        let bytes = self.render_bytes(context).await?;
        String::from_utf8(bytes.into()).map_err(RenderError::other)
    }

    /// Render to chunks with a variable value type
    async fn render_chunks_inner<Ctx, V>(
        &self,
        context: &Ctx,
    ) -> RenderedChunks<V>
    where
        Ctx: Context<V>,
        V: RenderValue,
    {
        // Map over each parsed chunk, and render the expressions into values.
        // because raw text uses Arc and expressions just contain metadata The
        // raw text chunks will be mapped 1:1. This clone is pretty cheap
        let futures = self.chunks.iter().map(|chunk| async move {
            match chunk {
                TemplateChunk::Raw(text) => {
                    RenderedChunk::Raw(Arc::clone(text))
                }
                TemplateChunk::Expression {
                    expression,
                    modifier,
                } => match expression.render(context).await {
                    Ok(value) => RenderedChunk::Dynamic(value),
                    Err(error) => RenderedChunk::Error(error),
                },
            }
        });

        // Concurrency!
        let chunks = future::join_all(futures).await;
        RenderedChunks(chunks)
    }
}

/// Build a single-chunk template
impl From<Expression> for Template {
    fn from(expression: Expression) -> Self {
        Self::from_chunks(vec![TemplateChunk::Expression {
            expression,
            modifier: None,
        }])
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
    Expression {
        /// Expression to render
        #[cfg_attr(
            test,
            proptest(strategy = "test_util::expression_arbitrary()")
        )]
        expression: Expression,
        /// Modifier changing the output of the expression
        modifier: Option<ExpressionModifier>,
    },
}

impl From<Expression> for TemplateChunk {
    fn from(expression: Expression) -> Self {
        Self::Expression {
            expression,
            modifier: None,
        }
    }
}

/// A modifier changes the behavior of a rendered expression
#[derive(Copy, Clone, Debug, PartialEq)]
#[cfg_attr(test, derive(proptest_derive::Arbitrary))]
pub enum ExpressionModifier {
    /// The rendered value from the expression should be unpacked as the value
    /// for the entire template
    ///
    /// For example:
    ///
    /// ```notrust
    /// "{{ 3 }}" => "3"
    /// "{{* 3 }}" => 3
    /// ```
    ///
    /// This is only allowed for templates with a single dynamic chunk, such
    /// as `{{* [1, 2, 3] }}`. Using this modifier in any other template will
    /// result in an error **at render time**. It'd be nice for the error to
    /// show up at parse time since it can be determined statically, but it's a
    /// bit complicated to ensure all template construction goes through that
    /// fallible path, and I'm being lazy.
    ///
    /// TODO add documentation for this
    Unpack,
}

impl Display for ExpressionModifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Unpack => MODIFIER_UNPACK,
        };
        write!(f, "{s}")
    }
}

/// Outcome of rendering the individual chunks of a template. This is an
/// intermediate output type that can be resolved into a variety of final
/// output types.
#[derive(Debug)]
pub struct RenderedChunks<V>(Vec<RenderedChunk<V>>);

impl<V: RenderValue> RenderedChunks<V> {
    /// Get the inner list of chunks
    pub fn into_chunks(self) -> Vec<RenderedChunk<V>> {
        self.0
    }

    /// Get the inner chunks as a slice
    pub fn chunks(&self) -> &[RenderedChunk<V>] {
        &self.0
    }

    /// Get an iterator over references to the chunks
    pub fn iter(&self) -> impl Iterator<Item = &RenderedChunk<V>> {
        self.0.iter()
    }

    /// Unpack this output into a single value
    ///
    /// If the output is a single dynamic chunk, unpack it into a scalar value.
    /// Otherwise, return `Err(self)`.
    pub fn unpack(self) -> Result<V, Self> {
        match <[_; 1]>::try_from(self.0) {
            // If we have a single dynamic chunk, return its value directly
            Ok([RenderedChunk::Dynamic(value)]) => Ok(value),
            // Unpack failed
            Ok(chunks @ [RenderedChunk::Raw(_) | RenderedChunk::Error(_)]) => {
                Err(Self(chunks.into()))
            }
            Err(chunks) => Err(Self(chunks)),
        }
    }
}

// Non-stream functions
impl RenderedChunks<Value> {
    /// Collect the rendered chunks into a [Value] by these rules:
    /// - If the template is a single dynamic chunk, return the output of that
    ///   chunk, which may be any type of [Value]
    /// - Any other template will be rendered to a string by stringifying each
    ///   dynamic chunk and concatenating them all together
    /// - If rendering to a string fails because the bytes are not valid UTF-8,
    ///   concatenate into a bytes object instead
    pub fn try_into_value(self) -> Result<Value, RenderError> {
        // If we only have one chunk, unpack it into a value
        let value = match self.unpack() {
            Ok(value) => value,
            Err(chunks) => {
                // Render to bytes
                let bytes = chunks.try_into_bytes()?;
                Value::Bytes(bytes)
            }
        };

        Ok(value.decode_bytes())
    }

    /// Collect the rendered chunks into a byte string
    ///
    /// If any chunk is an error, return an error.
    pub fn try_into_bytes(self) -> Result<Bytes, RenderError> {
        self.into_iter()
            .map(|chunk| match chunk {
                RenderedChunk::Raw(s) => {
                    Ok(Bytes::copy_from_slice(s.as_bytes()))
                }
                RenderedChunk::Dynamic(value) => Ok(value.into_bytes()),
                RenderedChunk::Error(error) => Err(error),
            })
            .flatten_ok()
            .try_collect()
    }
}

// Stream-only functions
impl RenderedChunks<ValueStream> {
    /// If this output is a single chunk and that chunk is a stream, get the
    /// source of the stream
    pub fn stream_source(&self) -> Option<&StreamSource> {
        if let [RenderedChunk::Dynamic(ValueStream::Stream { source, .. })] =
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
            RenderedChunk::Raw(_)
            | RenderedChunk::Dynamic(ValueStream::Value(_))
            | RenderedChunk::Error(_) => false,
            RenderedChunk::Dynamic(ValueStream::Stream { .. }) => true,
        })
    }

    /// Collect the rendered chunks into a [Value] by these rules:
    /// - If the template is a single dynamic chunk, return the output of that
    ///   chunk, which may be any type of [Value]
    /// - If there are any streams, resolve them to bytes
    /// - Any other template will be rendered to a string by stringifying each
    ///   dynamic chunk and concatenating them all together
    /// - If rendering to a string fails because the bytes are not valid UTF-8,
    ///   concatenate into a bytes object instead
    pub async fn try_collect_value(self) -> Result<Value, RenderError> {
        // If we only have one chunk, unpack it into a value
        let value = match self.unpack() {
            Ok(ValueStream::Value(value)) => value,
            Ok(stream @ ValueStream::Stream { .. }) => stream.resolve().await?,
            Err(chunks) => {
                // Render to bytes
                let bytes = chunks
                    .try_into_stream()?
                    .try_collect::<BytesMut>()
                    .await?
                    .into();
                Value::Bytes(bytes)
            }
        };

        // Try to convert bytes to string, because that's generally more
        // useful to the consumer
        Ok(value.decode_bytes())
    }

    /// Convert this output into a byte stream. Each chunk will be yielded as a
    /// separate `Bytes` output from the stream, except for inner stream chunks,
    /// which can yield any number of values based on their implementation.
    /// Return `Err` if any of the rendered chunks are errors.
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
                RenderedChunk::Dynamic(value) => match value {
                    ValueStream::Value(value) => {
                        stream_value(value.into_bytes())
                    }
                    ValueStream::Stream { stream, .. } => Ok(stream.boxed()),
                },

                RenderedChunk::Error(error) => Err(error),
            })
            .collect::<Result<Vec<_>, _>>()?;

        // If none of the chunks failed, we can chain all the streams together
        Ok(stream::iter(chunks).flatten())
    }
}

/// Create render output of a single chunk with a value
impl<V: From<Value>> From<Value> for RenderedChunks<V> {
    fn from(value: Value) -> Self {
        Self(vec![RenderedChunk::Dynamic(value.into())])
    }
}

/// Create render output of a single chunk that may have failed
impl<V: From<Value>> From<Result<Value, RenderError>> for RenderedChunks<V> {
    fn from(result: Result<Value, RenderError>) -> Self {
        let chunk = match result {
            Ok(value) => RenderedChunk::Dynamic(value.into()),
            Err(error) => RenderedChunk::Error(error),
        };
        Self(vec![chunk])
    }
}

/// Get an iterator over the chunks of this output
impl<V> IntoIterator for RenderedChunks<V> {
    type Item = RenderedChunk<V>;
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

/// Get an iterator over references to chunks of this output
impl<'a, V> IntoIterator for &'a RenderedChunks<V> {
    type Item = &'a RenderedChunk<V>;
    type IntoIter = slice::Iter<'a, RenderedChunk<V>>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

/// A piece of a rendered template string. A collection of chunks collectively
/// constitutes a rendered string when displayed contiguously.
#[derive(Debug)]
pub enum RenderedChunk<V> {
    /// Raw unprocessed text, i.e. something **outside** the `{{ }}`. This is
    /// stored in an `Arc` so we can reference the text in the parsed input
    /// without having to clone it.
    Raw(Arc<str>),
    /// A dynamic chunk of a template, rendered to a stream/value
    Dynamic(V),
    /// An error occurred while rendering a template key
    Error(RenderError),
}

#[cfg(test)]
impl<V: PartialEq> PartialEq for RenderedChunk<V> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Raw(raw1), Self::Raw(raw2)) => raw1 == raw2,
            (Self::Dynamic(value1), Self::Dynamic(value2)) => value1 == value2,
            (Self::Error(error1), Self::Error(error2)) => {
                // RenderError doesn't have a PartialEq impl, so we have to
                // do a string comparison.
                error1.to_string() == error2.to_string()
            }
            _ => false,
        }
    }
}
