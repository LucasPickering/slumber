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

pub use error::{RenderError, TemplateParseError};
pub use expression::{Expression, FunctionCall, Identifier, Literal};

use crate::parse::{FALSE, NULL, TRUE};
use bytes::{Bytes, BytesMut};
use derive_more::From;
use futures::future;
use indexmap::IndexMap;
#[cfg(test)]
use proptest::{arbitrary::any, strategy::Strategy};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, VecDeque},
    fmt::Debug,
    sync::Arc,
};

/// `Context` defines how template fields and functions are resolved. Both
/// field resolution and function calls can be asynchronous.
pub trait Context: Sized + Send + Sync {
    /// Get the value of a field from the context. The implementor can decide
    /// where fields are derived from. Fields can also be computed dynamically
    /// and be `async`. For example, fields can be loaded from a map of nested
    /// templates, in which case the nested template would need to be rendered
    /// before this can be returned.
    fn get(
        &self,
        identifier: &Identifier,
    ) -> impl Future<Output = Result<Value, RenderError>> + Send;

    /// Call a function by name
    fn call(
        &self,
        function_name: &Identifier,
        arguments: Arguments<'_, Self>,
    ) -> impl Future<Output = Result<Value, RenderError>> + Send;
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
/// - No two raw segments will ever be consecutive
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
        Self::function_call("file", [path], [] as [(&str, Expression); 0])
    }

    /// Create a new template that contains a single chunk, which is an
    /// expression that invokes a function with arguments.
    ///
    /// ```
    /// use slumber_template::Template;
    ///
    /// let template =
    ///     Template::function_call("hello", ["john"], [("mode", "caps")]);
    /// assert_eq!(template.display(), "{{ hello('john', mode='caps') }}");
    /// ```
    pub fn function_call(
        name: &'static str,
        position: impl IntoIterator<Item = impl Into<Expression>>,
        keyword: impl IntoIterator<Item = (&'static str, impl Into<Expression>)>,
    ) -> Self {
        let chunks =
            vec![TemplateChunk::Expression(Expression::Call(FunctionCall {
                function: name.into(),
                position: position.into_iter().map(Into::into).collect(),
                keyword: keyword
                    .into_iter()
                    .map(|(key, value)| (key.into(), value.into()))
                    .collect(),
            }))];
        Self { chunks }
    }

    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }

    /// Render the template using values from the given context. If any chunk
    /// failed to render, return an error. The template is rendered as bytes,
    /// meaning it can safely render to non-UTF-8 content. Use
    /// [Self::render_string] if you want the bytes converted to a string.
    pub async fn render_bytes<Ctx: Context>(
        &self,
        context: &Ctx,
    ) -> Result<Bytes, RenderError> {
        let chunks = self.render_chunks(context).await;

        // Take an educated guess at the needed capacity to avoid reallocations
        let capacity = chunks
            .iter()
            .map(|chunk| match chunk {
                RenderedChunk::Raw(s) => s.len(),
                RenderedChunk::Rendered(Value::Bytes(bytes)) => bytes.len(),
                RenderedChunk::Rendered(Value::String(s)) => s.len(),
                // Take a rough guess for anything other than bytes/string
                RenderedChunk::Rendered(_) => 5,
                RenderedChunk::Error(_) => 0,
            })
            .sum();
        chunks
            .into_iter()
            .try_fold(BytesMut::with_capacity(capacity), |mut acc, chunk| {
                match chunk {
                    RenderedChunk::Raw(s) => acc.extend(s.as_bytes()),
                    RenderedChunk::Rendered(Value::Bytes(bytes)) => {
                        acc.extend(bytes);
                    }
                    RenderedChunk::Rendered(value) => {
                        let s = value.try_into_string()?;
                        acc.extend(s.into_bytes());
                    }
                    RenderedChunk::Error(error) => return Err(error),
                }
                Ok(acc)
            })
            .map(Bytes::from)
    }

    /// Render the template using values from the given context. If any chunk
    /// failed to render, return an error. The rendered template will be
    /// converted from raw bytes to UTF-8. If it is not valid UTF-8, return an
    /// error.
    pub async fn render_string<Ctx: Context>(
        &self,
        context: &Ctx,
    ) -> Result<String, RenderError> {
        let bytes = self.render_bytes(context).await?;
        String::from_utf8(bytes.into()).map_err(RenderError::other)
    }

    /// Render the template using values from the given context, returning the
    /// individual rendered chunks rather than stitching them together into a
    /// string. If any individual chunk fails to render, its error will be
    /// returned inline as [RenderedChunk::Error] and the rest of the template
    /// will still be rendered.
    pub async fn render_chunks<Ctx: Context>(
        &self,
        context: &Ctx,
    ) -> Vec<RenderedChunk> {
        // Map over each parsed chunk, and render the expressions into values.
        // because raw text uses Arc and expressions just contain metadata
        // The raw text chunks will be mapped 1:1. This clone is pretty cheap
        let futures = self.chunks.iter().map(|chunk| async move {
            match chunk {
                TemplateChunk::Raw(text) => {
                    RenderedChunk::Raw(Arc::clone(text))
                }
                TemplateChunk::Expression(expression) => expression
                    .render(context)
                    .await
                    .map_or_else(RenderedChunk::Error, RenderedChunk::Rendered),
            }
        });

        // Concurrency!
        future::join_all(futures).await
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

#[cfg(any(test, feature = "test"))]
impl From<serde_json::Value> for Template {
    fn from(value: serde_json::Value) -> Self {
        format!("{value:#}").into()
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

/// A runtime template value. This very similar to a JSON value, except:
/// - Numbers do not support arbitrary size
/// - Bytes are supported
#[derive(Clone, Debug, From, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Bytes(Bytes),
    Array(Vec<Self>),
    Object(IndexMap<String, Self>),
}

impl Value {
    /// Convert this value to a boolean, according to its truthiness.
    /// Truthiness/falsiness is defined for each type as:
    /// - `null` - `false`
    /// - `bool` - Own value
    /// - `int` - `false` if zero
    /// - `float` - `false` if zero
    /// - `string` - `false` if empty
    /// - `bytes` - `false` if empty
    /// - `array` - `false` if empty
    /// - `object` - `false` if empty
    ///
    /// These correspond to the truthiness rules from Python.
    pub fn to_bool(&self) -> bool {
        match self {
            Self::Null => false,
            Self::Bool(b) => *b,
            Self::Int(i) => *i != 0,
            Self::Float(f) => *f != 0.0,
            Self::String(s) => !s.is_empty(),
            Self::Bytes(bytes) => !bytes.is_empty(),
            Self::Array(array) => !array.is_empty(),
            Self::Object(object) => !object.is_empty(),
        }
    }

    /// Attempt to convert this value to a string. This can fail only if the
    /// value contains non-UTF-8 bytes, or if it is a collection that contains
    /// non-UTF-8 bytes.
    pub fn try_into_string(self) -> Result<String, RenderError> {
        match self {
            Self::Null => Ok(NULL.into()),
            Self::Bool(false) => Ok(FALSE.into()),
            Self::Bool(true) => Ok(TRUE.into()),
            Self::Int(i) => Ok(i.to_string()),
            Self::Float(f) => Ok(f.to_string()),
            Self::String(s) => Ok(s),
            Self::Bytes(bytes) => {
                String::from_utf8(bytes.into()).map_err(RenderError::from)
            }
            // Use the display impl
            Self::Array(_) | Self::Object(_) => Ok(self.to_string()),
        }
    }

    /// Convert a JSON value to a template value. This is infallible because
    /// [Value] is a superset of JSON
    pub fn from_json(json: serde_json::Value) -> Self {
        serde_json::from_value(json).unwrap()
    }
}

impl From<&Literal> for Value {
    fn from(literal: &Literal) -> Self {
        match literal {
            Literal::Null => Value::Null,
            Literal::Bool(b) => Value::Bool(*b),
            Literal::Int(i) => Value::Int(*i),
            Literal::Float(f) => Value::Float(*f),
            Literal::String(s) => Value::String(s.clone()),
            Literal::Bytes(bytes) => Value::Bytes(bytes.clone()),
        }
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
    /// Outcome of rendering a template key
    Rendered(Value),
    /// An error occurred while rendering a template key
    Error(RenderError),
}

#[cfg(test)]
impl PartialEq for RenderedChunk {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Raw(raw1), Self::Raw(raw2)) => raw1 == raw2,
            (Self::Rendered(value1), Self::Rendered(value2)) => {
                value1 == value2
            }
            (Self::Error(error1), Self::Error(error2)) => {
                // RenderError doesn't have a PartialEq impl, so we have to
                // do a string comparison.
                error1.to_string() == error2.to_string()
            }
            _ => false,
        }
    }
}

/// Arguments passed to a function call
///
/// This container holds all the data a template function may need to construct
/// its own arguments. All given positional and keyword arguments are expected
/// to be used, and [assert_consumed](Self::assert_consumed) should be called
/// after extracting arguments to ensure no additional ones were passed.
#[derive(Debug)]
pub struct Arguments<'ctx, Ctx> {
    /// Arbitrary user-provided context available to every template render and
    /// function call
    pub(crate) context: &'ctx Ctx,
    /// Position arguments. This queue will be drained from the front as
    /// arguments are converted, and additional arguments not accepted by the
    /// function will trigger an error.
    pub(crate) position: VecDeque<Value>,
    /// Keyword arguments. These will be converted wholesale as a single map,
    /// as there's no Rust support for kwargs. All keyword arguments are
    /// optional.
    pub(crate) keyword: HashMap<String, Value>,
}

impl<'ctx, Ctx> Arguments<'ctx, Ctx> {
    /// Get a reference to the template context
    pub fn context(&self) -> &'ctx Ctx {
        self.context
    }

    /// Pop the next positional argument off the front of the queue and convert
    /// it to type `T` using its [TryFromValue] implementation. Return an error
    /// if there are no positional arguments left or the conversion fails.
    pub fn pop_position<T: TryFromValue>(&mut self) -> Result<T, RenderError> {
        let value = self
            .position
            .pop_front()
            .ok_or(RenderError::NotEnoughArguments)?;
        T::try_from_value(value)
    }

    /// Remove a keyword argument from the argument set, converting it to type
    /// `T` using its [TryFromValue] implementation. Return an error if the
    /// keyword argument does not exist or the conversion fails.
    pub fn pop_keyword<T: Default + TryFromValue>(
        &mut self,
        name: &str,
    ) -> Result<T, RenderError> {
        match self.keyword.remove(name) {
            Some(value) => T::try_from_value(value),
            // Kwarg not provided - use the default value
            None => Ok(T::default()),
        }
    }

    /// Ensure that all positional and keyword arguments have been consumed.
    /// Return an error if any arguments were passed by the user but not
    /// consumed by the function implementation.
    pub fn ensure_consumed(self) -> Result<(), RenderError> {
        if self.position.is_empty() && self.keyword.is_empty() {
            Ok(())
        } else {
            Err(RenderError::TooManyArguments {
                position: self.position.into(),
                keyword: self.keyword,
            })
        }
    }
}

/// Convert [Value] to a type fallibly
///
/// This is used for converting function arguments to the static types expected
/// by the function implementations.
pub trait TryFromValue: Sized {
    fn try_from_value(value: Value) -> Result<Self, RenderError>;
}

impl TryFromValue for Value {
    fn try_from_value(value: Value) -> Result<Self, RenderError> {
        Ok(value)
    }
}

impl TryFromValue for bool {
    fn try_from_value(value: Value) -> Result<Self, RenderError> {
        Ok(value.to_bool())
    }
}

impl TryFromValue for String {
    fn try_from_value(value: Value) -> Result<Self, RenderError> {
        // This will succeed for anything other than invalid UTF-8 bytes
        value.try_into_string()
    }
}

impl<T> TryFromValue for Option<T>
where
    T: TryFromValue,
{
    fn try_from_value(value: Value) -> Result<Self, RenderError> {
        if let Value::Null = value {
            Ok(None)
        } else {
            T::try_from_value(value).map(Some)
        }
    }
}

/// Convert an array to a list
impl<T> TryFromValue for Vec<T>
where
    T: TryFromValue,
{
    fn try_from_value(value: Value) -> Result<Self, RenderError> {
        if let Value::Array(array) = value {
            array.into_iter().map(T::try_from_value).collect()
        } else {
            Err(RenderError::Type {
                expected: "array",
                actual: value,
            })
        }
    }
}

/// Convert a template value to JSON. If the value is bytes, this will
/// deserialize it as JSON, otherwise it will convert directly. This allows us
/// to parse response bodies as JSON while accepting anything else as a native
/// JSON value
impl TryFromValue for serde_json::Value {
    fn try_from_value(value: Value) -> Result<Self, RenderError> {
        match value {
            Value::Null => Ok(serde_json::Value::Null),
            Value::Bool(b) => Ok(b.into()),
            Value::Int(i) => Ok(i.into()),
            Value::Float(f) => Ok(f.into()),
            Value::String(s) => Ok(s.into()),
            Value::Array(array) => array
                .into_iter()
                .map(serde_json::Value::try_from_value)
                .collect(),
            Value::Object(map) => map
                .into_iter()
                .map(|(k, v)| Ok((k, serde_json::Value::try_from_value(v)?)))
                .collect(),
            Value::Bytes(bytes) => {
                // Assume this is an encoded JSON string and deserialize it
                serde_json::from_slice(&bytes).map_err(|error| {
                    RenderError::JsonDeserialize { data: bytes, error }
                })
            }
        }
    }
}

/// Implement [TryFromValue] for the given type by converting the [Value] to a
/// [String], then using `T`'s [FromStr] implementation to convert to `T`.
///
/// This could be a derive macro, but decl is much simpler
#[macro_export]
macro_rules! impl_try_from_value_str {
    ($type:ty) => {
        impl TryFromValue for $type {
            fn try_from_value(
                value: $crate::Value,
            ) -> Result<Self, RenderError> {
                let s = String::try_from_value(value)?;
                s.parse().map_err(RenderError::other)
            }
        }
    };
}

/// Convert any value into `Result<Value, RenderError>`
///
/// This is used for converting function outputs back to template values.
pub trait FunctionOutput {
    fn into_result(self) -> Result<Value, RenderError>;
}

impl<T> FunctionOutput for T
where
    Value: From<T>,
{
    fn into_result(self) -> Result<Value, RenderError> {
        Ok(self.into())
    }
}

impl<T, E> FunctionOutput for Result<T, E>
where
    T: Into<Value> + Send + Sync,
    E: Into<RenderError> + Send + Sync,
{
    fn into_result(self) -> Result<Value, RenderError> {
        self.map(T::into).map_err(E::into)
    }
}

impl<T: FunctionOutput> FunctionOutput for Option<T> {
    fn into_result(self) -> Result<Value, RenderError> {
        self.map(T::into_result).unwrap_or(Ok(Value::Null))
    }
}
