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
mod util;
mod value;

pub use error::{
    Expected, RenderError, TemplateParseError, ValueError, WithValue,
};
pub use expression::{Expression, FunctionCall, Identifier, Literal};
pub use util::FieldCache;
pub use value::{
    Arguments, FunctionOutput, LazyValue, StreamSource, TryFromValue, Value,
};

use bytes::{Bytes, BytesMut};
use futures::{Stream, StreamExt, future, stream};
use itertools::Itertools;
#[cfg(test)]
use proptest::{arbitrary::any, strategy::Strategy};
use slumber_util::NEW_ISSUE_LINK;
use std::{fmt::Debug, sync::Arc};

/// `Context` defines how template fields and functions are resolved. Both
/// field resolution and function calls can be asynchronous.
pub trait Context: Sized + Send + Sync {
    /// Does the render target support streaming? Typically this should return
    /// `false`. To enable streaming, just call [Template::render_stream] and
    /// the context will be wrapped to enable streaming.
    ///
    /// This is a method on the context to avoid plumbing around a second object
    /// to all render locations.
    fn can_stream(&self) -> bool {
        false
    }

    /// Get the value of a field from the context. The implementor can decide
    /// where fields are derived from. Fields can also be computed dynamically
    /// and be `async`. For example, fields can be loaded from a map of nested
    /// templates, in which case the nested template would need to be rendered
    /// before this can be returned. Rendered fields will be cached via the
    /// cache returned by [Self::field_cache], so the same field will never be
    /// requested twice for this context object.
    fn get_field(
        &self,
        identifier: &Identifier,
    ) -> impl Future<Output = Result<LazyValue, RenderError>> + Send;

    /// A cache to store the outcome of rendered fields.
    fn field_cache(&self) -> &FieldCache;

    /// Call a function by name
    fn call(
        &self,
        function_name: &Identifier,
        arguments: Arguments<'_, Self>,
    ) -> impl Future<Output = Result<LazyValue, RenderError>> + Send;
}

/// A wrapper for a [Context] implementation that enables streaming all other
/// behavior is forwarded to the inner context. This is automatically applied by
/// [Template::render], but can also be used manually to control the
/// output of [Template::render_chunks].
///
/// TODO update comment
#[derive(Debug)]
struct StreamContext<'a, T> {
    context: &'a T,
    can_stream: bool,
}

impl<T: Context> Context for StreamContext<'_, T> {
    fn can_stream(&self) -> bool {
        self.can_stream
    }

    async fn get_field(
        &self,
        identifier: &Identifier,
    ) -> Result<LazyValue, RenderError> {
        self.context.get_field(identifier).await
    }

    fn field_cache(&self) -> &FieldCache {
        self.context.field_cache()
    }

    async fn call(
        &self,
        function_name: &Identifier,
        arguments: Arguments<'_, Self>,
    ) -> Result<LazyValue, RenderError> {
        self.context
            .call(function_name, arguments.map_context(|ctx| ctx.context))
            .await
    }
}

/// A parsed template, which can contain raw and/or templated content. The
/// string is parsed during creation to identify template keys, hence the
/// immutability.
///
/// The original string is *not* stored. To recover the source string, use the
/// [Display] implementation.
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

    /// Render the template, returning the individual rendered chunks rather
    /// than stitching them together into a string. If any individual chunk
    /// fails to render, its error will be returned inline as
    /// [RenderedChunk::Error] and the rest of the template will still be
    /// rendered.
    ///
    /// TODO update comment
    pub async fn render<Ctx: Context>(
        &self,
        context: &Ctx,
        can_stream: bool,
    ) -> RenderedChunks {
        // TODO should we pass the futures directly into RenderedChunks so we
        // can start streaming before they're all rendered?

        // Map over each parsed chunk, and render the expressions into values.
        // because raw text uses Arc and expressions just contain metadata
        // The raw text chunks will be mapped 1:1. This clone is pretty cheap
        let futures = self.chunks.iter().map(|chunk| async move {
            let context = StreamContext {
                context,
                can_stream,
            };
            match chunk {
                TemplateChunk::Raw(text) => {
                    RenderedChunk::Raw(Arc::clone(text))
                }
                TemplateChunk::Expression(expression) => {
                    match expression.render(&context).await {
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
        RenderedChunks(chunks)
    }

    /// Render the template. If any chunk fails to render, return an error. The
    /// render output is converted to a [Value] by these rules:
    /// - If the template is a single dynamic chunk, return the output of that
    ///   chunk, which may be any type of [Value]
    /// - Any other template will be rendered to a string by stringifying each
    ///   dynamic chunk and concatenating them all together
    /// - If rendering to a string fails because the bytes are not valid UTF-8,
    ///   concatenate into a bytes object instead
    ///
    /// Return an error iff any chunk fails to render. This will never fail on
    /// output conversion because it can always fall back to returning raw
    /// bytes.
    ///
    /// TODO roll this into try_into_value()
    pub async fn render_value<Ctx: Context>(
        &self,
        context: &Ctx,
    ) -> Result<Value, RenderError> {
        let chunks = self.render(context, false).await;
        chunks.try_into_value().await
    }

    /// Render the template. If any chunk fails to render, return an error. The
    /// output is returned as bytes, meaning it can safely render to non-UTF-8
    /// content. Use [Self::render_string] if you want the bytes converted to a
    /// string.
    pub async fn render_bytes<Ctx: Context>(
        &self,
        context: &Ctx,
    ) -> Result<Bytes, RenderError> {
        self.render(context, false).await.try_into_bytes().await
    }

    /// Render the template. If any chunk fails to render, return an error. The
    /// output will be converted from raw bytes to UTF-8. If it is not valid
    /// UTF-8, return an error.
    pub async fn render_string<Ctx: Context>(
        &self,
        context: &Ctx,
    ) -> Result<String, RenderError> {
        let bytes = self.render_bytes(context).await?;
        String::from_utf8(bytes.into()).map_err(RenderError::other)
    }
}

#[cfg(any(test, feature = "test"))]
impl From<&str> for Template {
    fn from(value: &str) -> Self {
        value.parse().unwrap()
    }
}

#[cfg(any(test, feature = "test"))]
impl From<String> for Template {
    fn from(value: String) -> Self {
        value.as_str().into()
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

/// TODO doc
/// TODO rename
#[derive(Debug)]
pub struct RenderedChunks(Vec<RenderedChunk>);

impl RenderedChunks {
    /// TODO
    pub fn try_into_stream(
        self,
    ) -> Result<impl Stream<Item = Result<Bytes, RenderError>>, RenderError>
    {
        // TODO explain
        // TODO can we do this without boxing?
        let mut stream = stream::empty().boxed();
        for chunk in self.0 {
            match chunk {
                RenderedChunk::Raw(s) => {
                    // TODO remove the future
                    let chunk_stream = stream::once(async move {
                        Ok(Bytes::from(s.as_bytes().to_owned()))
                    });
                    stream = stream.chain(chunk_stream).boxed();
                }
                RenderedChunk::Rendered(value) => {
                    stream = stream.chain(value.into_stream()).boxed();
                }
                RenderedChunk::Error(error) => return Err(error),
            }
        }
        Ok(stream)
    }

    /// TODO
    pub async fn try_into_lazy(mut self) -> Result<LazyValue, RenderError> {
        if let &[RenderedChunk::Rendered(_)] = self.0.as_slice() {
            // If we have a single dynamic chunk, return its value directly
            let Some(RenderedChunk::Rendered(value)) = self.0.pop() else {
                // Checked pattern above
                unreachable!()
            };
            Ok(value)
        } else {
            // Render to bytes
            let bytes = self.try_into_bytes().await?;
            Ok(Value::Bytes(bytes).into())
        }
    }

    /// TODO
    pub async fn try_into_value(self) -> Result<Value, RenderError> {
        let value = self.try_into_lazy().await?.resolve().await?;

        // Try to convert bytes to string, because that's generally more useful
        // to the consumer
        match value {
            Value::Bytes(bytes) => match String::from_utf8(bytes.into()) {
                Ok(s) => Ok(Value::String(s)),
                Err(error) => Ok(Value::Bytes(error.into_bytes().into())),
            },
            _ => Ok(value),
        }
    }

    /// Concatenate rendered chunks into bytes. If any chunk is an error, return
    /// an error. This is async because the chunk may be a stream, in which
    /// case it will be resolved.
    pub async fn try_into_bytes(self) -> Result<Bytes, RenderError> {
        // TODO we could just collect the stream here instead

        // Take an educated guess at the needed capacity to avoid reallocations
        let capacity = self
            .0
            .iter()
            .map(|chunk| match chunk {
                RenderedChunk::Raw(s) => s.len(),
                RenderedChunk::Rendered(LazyValue::Value(Value::Bytes(
                    bytes,
                ))) => bytes.len(),
                RenderedChunk::Rendered(LazyValue::Value(Value::String(s))) => {
                    s.len()
                }
                // Take a rough guess for anything other than bytes/string
                RenderedChunk::Rendered(_) => 5,
                RenderedChunk::Error(_) => 0,
            })
            .sum();

        let mut bytes = BytesMut::with_capacity(capacity);
        for chunk in self.0 {
            match chunk {
                RenderedChunk::Raw(s) => bytes.extend(s.as_bytes()),
                RenderedChunk::Rendered(stream) => {
                    // If the chunk is still a stream, resolve to a value now
                    let value = stream.resolve().await?;
                    bytes.extend(value.into_bytes());
                }
                RenderedChunk::Error(error) => return Err(error),
            }
        }
        Ok(bytes.into())
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
