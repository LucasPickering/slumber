use crate::render::TemplateContext;
use bytes::BytesMut;
use futures::{FutureExt, TryStreamExt, future};
use serde::{Serialize, Serializer};
use slumber_template::{
    Context, Expression, RenderError, RenderValue, RenderedChunk,
    RenderedChunks, Template, TemplateChunk, TemplateParseError, Value,
    ValueStream,
};
use std::str::FromStr;

/// A templated [Value]
///
/// This is a [Value], except the strings are templates. That means this can be
/// dynamically rendered to a [Value]. This is used for structured bodies (e.g.
/// JSON) as well as profile fields.
#[derive(Clone, Debug, derive_more::From, PartialEq, Serialize)]
#[serde(untagged)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum ValueTemplate {
    Null,
    Boolean(bool),
    Integer(i64),
    Float(f64),
    /// An unpacked single-chunk template
    #[serde(serialize_with = "serialize_expression")]
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    Expression(Expression),
    /// A template that has not been unpacked
    ///
    /// This is either a single raw chunk or a multi-chunk template
    #[from(ignore)]
    String(Template),
    #[from(ignore)]
    Array(Vec<Self>),
    // A key-value mapping. Stored as a `Vec` instead of `IndexMap` because
    // the keys are templates, which aren't hashable. We never do key lookups
    // on this so there's no need for a map anyway.
    #[from(ignore)]
    #[serde(serialize_with = "slumber_util::serialize_mapping")]
    #[cfg_attr(
        feature = "schema",
        schemars(with = "std::collections::HashMap<Template, Self>")
    )]
    Object(Vec<(Template, Self)>),
}

impl ValueTemplate {
    /// Create a new string template from a raw string, without parsing it at
    /// all
    ///
    /// Useful when importing from external formats where the string isn't
    /// expected to be a valid Slumber template
    pub fn raw(template: String) -> Self {
        Self::String(Template::raw(template))
    }

    /// Does the template have at least one dynamic chunk? If this returns
    /// `false`, the template will always render to its source text
    pub fn is_dynamic(&self) -> bool {
        match self {
            Self::Null
            | Self::Boolean(_)
            | Self::Integer(_)
            | Self::Float(_) => false,
            Self::Expression(_) => true,
            Self::String(template) => template.is_dynamic(),
            Self::Array(array) => array.iter().any(Self::is_dynamic),
            Self::Object(object) => object
                .iter()
                .any(|(key, value)| key.is_dynamic() || value.is_dynamic()),
        }
    }

    /// Render to a value or chunks with eager values
    ///
    /// The return value is *usually* a single value, but if `self` is a
    /// multi-chunk template string, then its multi-chunk output will be the
    /// output for this.
    ///
    /// Use this for cases where streaming is *not* allowed.
    pub async fn render_value(
        &self,
        context: &TemplateContext,
    ) -> RenderedValue<Value> {
        self.render_chunks_inner(context, Template::render_chunks)
            .boxed_local() // Box for recursion
            .await
    }

    /// Render to a value or chunks with stream values
    ///
    /// The return value is *usually* a single value, but if `self` is a
    /// multi-chunk template string, then its multi-chunk output will be the
    /// output for this.
    ///
    /// Use this for cases where streaming is allowed.
    pub async fn render_value_stream(
        &self,
        context: &TemplateContext,
    ) -> RenderedValue<ValueStream> {
        self.render_chunks_inner(context, Template::render_chunks_stream)
            .boxed_local() // Box for recursion
            .await
    }

    /// Render to chunks with dynamic output type
    async fn render_chunks_inner<V>(
        &self,
        context: &TemplateContext,
        render_string: impl AsyncFn(
            &Template,
            &TemplateContext,
        ) -> RenderedChunks<V>,
    ) -> RenderedValue<V>
    where
        V: RenderValue,
        TemplateContext: Context<V>,
    {
        match self {
            Self::Null => Value::Null.into(),
            Self::Boolean(b) => Value::Boolean(*b).into(),
            Self::Integer(i) => Value::Integer(*i).into(),
            Self::Float(f) => Value::Float(*f).into(),
            Self::Expression(expression) => {
                let result = expression.render(context).await;
                RenderedValue::Value(result)
            }
            Self::String(template) => {
                RenderedValue::Chunks(render_string(template, context).await)
            }
            Self::Array(array) => {
                // Render each value and collection into an Array
                future::try_join_all(array.iter().map(|value| {
                    value
                        .render_value(context)
                        .map(RenderedValue::try_into_value)
                }))
                .await
                .map(Value::Array)
                .into() // Wrap into RenderedValue
            }
            Self::Object(map) => {
                // Render each key/value and collect into an Object
                future::try_join_all(map.iter().map(|(key, value)| async {
                    let key = key.render_string(context).await?;
                    let value =
                        value.render_value(context).await.try_into_value()?;
                    Ok::<_, RenderError>((key, value))
                }))
                .await
                .map(Value::from)
                .into() // Wrap into RenderedValue
            }
        }
    }
}

/// Parse template from a string literal. Panic if invalid
#[cfg(any(test, feature = "test"))]
impl From<&str> for ValueTemplate {
    fn from(value: &str) -> Self {
        value.parse().unwrap()
    }
}

#[cfg(any(test, feature = "test"))]
impl<T: Into<ValueTemplate>> From<Vec<T>> for ValueTemplate {
    fn from(value: Vec<T>) -> Self {
        Self::Array(value.into_iter().map(T::into).collect())
    }
}

#[cfg(any(test, feature = "test"))]
impl<T: Into<ValueTemplate>> From<Vec<(&str, T)>> for ValueTemplate {
    fn from(value: Vec<(&str, T)>) -> Self {
        Self::Object(
            value
                .into_iter()
                .map(|(k, v)| (k.parse().unwrap(), v.into()))
                .collect(),
        )
    }
}

/// A value rendered from a [ValueTemplate]
#[derive(Debug)]
pub enum RenderedValue<V> {
    /// A single-chunk dynamic template unpacked to a single value (or error)
    Value(Result<V, RenderError>),
    /// A raw or multi-chunk template that can be stitched into a string
    Chunks(RenderedChunks<V>),
}

impl RenderedValue<Value> {
    /// Collect the rendered value into a [Value] by these rules:
    /// - If the value is a [RenderedValue::Value], return that value directly
    /// - Any other template will be rendered to a string by stringifying each
    ///   dynamic chunk and concatenating them all together
    /// - If rendering to a string fails because the bytes are not valid UTF-8,
    ///   concatenate into a bytes object instead
    pub fn try_into_value(self) -> Result<Value, RenderError> {
        let value = match self {
            RenderedValue::Value(result) => result?,
            RenderedValue::Chunks(chunks) => {
                // Render to bytes
                let bytes = chunks.try_into_bytes()?;
                Value::Bytes(bytes)
            }
        };
        Ok(value.decode_bytes())
    }
}

impl RenderedValue<ValueStream> {
    /// Does this output contain *any* stream chunks?
    pub fn has_stream(&self) -> bool {
        match self {
            Self::Value(Ok(ValueStream::Stream { .. })) => true,
            Self::Value(Ok(ValueStream::Value(_)) | Err(_)) => false,
            Self::Chunks(chunks) => chunks.iter().any(|chunk| match chunk {
                RenderedChunk::Raw(_)
                | RenderedChunk::Dynamic(ValueStream::Value(_))
                | RenderedChunk::Error(_) => false,
                RenderedChunk::Dynamic(ValueStream::Stream { .. }) => true,
            }),
        }
    }

    /// Collect the rendered chunks into a [Value] by these rules:
    /// - If the value is a [RenderedValue::Value], return that value directly
    /// - If there are any streams, resolve them to bytes
    /// - Any other template will be rendered to a string by stringifying each
    ///   dynamic chunk and concatenating them all together
    /// - If rendering to a string fails because the bytes are not valid UTF-8,
    ///   concatenate into a bytes object instead
    pub async fn try_collect_value(self) -> Result<Value, RenderError> {
        // If we only have one chunk, unpack it into a value
        let value = match self {
            Self::Value(Ok(ValueStream::Value(value))) => value,
            Self::Value(Ok(stream @ ValueStream::Stream { .. })) => {
                stream.resolve().await?
            }
            Self::Value(Err(error)) => return Err(error),
            Self::Chunks(chunks) => {
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
}

/// Create render output of a single chunk with a value
impl<V: RenderValue> From<Value> for RenderedValue<V> {
    fn from(value: Value) -> Self {
        Self::Value(Ok(V::from_value(value)))
    }
}

/// Create render output of a single chunk that may have failed
impl<V: RenderValue> From<Result<Value, RenderError>> for RenderedValue<V> {
    fn from(result: Result<Value, RenderError>) -> Self {
        Self::Value(result.map(V::from_value))
    }
}

impl From<Template> for ValueTemplate {
    fn from(template: Template) -> Self {
        // Single dynamic chunk is unpacked
        match <[_; 1]>::try_from(template.into_chunks()) {
            Ok([TemplateChunk::Expression(expression)]) => {
                Self::Expression(expression)
            }
            Ok(chunks) => Self::String(Template::from_chunks(chunks.into())),
            Err(chunks) => Self::String(Template::from_chunks(chunks)),
        }
    }
}

/// Parse a template string to [ValueTemplate]
///
/// This will unpack the template to an expression if possible.
impl FromStr for ValueTemplate {
    type Err = TemplateParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let template: Template = s.parse()?;
        Ok(template.into())
    }
}

/// Serialize an expression as `{{ expression }}`
fn serialize_expression<S: Serializer>(
    expression: &Expression,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&Template::display_expression(expression))
}
