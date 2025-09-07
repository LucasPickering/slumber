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

pub use error::{
    Expected, RenderError, TemplateParseError, ValueError, WithValue,
};
pub use expression::{Expression, FunctionCall, Identifier, Literal};

use crate::{
    error::RenderErrorContext,
    parse::{FALSE, NULL, TRUE},
};
use bytes::{Bytes, BytesMut};
use derive_more::From;
use futures::future;
use indexmap::IndexMap;
use itertools::Itertools;
#[cfg(test)]
use proptest::{arbitrary::any, strategy::Strategy};
use serde::{Deserialize, Serialize};
use slumber_util::NEW_ISSUE_LINK;
use std::{collections::VecDeque, fmt::Debug, sync::Arc};

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

    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }

    /// Render the template using values from the given context. If any chunk
    /// failed to render, return an error. The render output is converted to a
    /// [Value] by these rules:
    /// - If the template is a single dynamic chunk, the output value will be
    ///   directly converted to JSON, allowing non-string JSON values
    /// - Any other template will be rendered to a string by stringifying each
    ///   dynamic chunk and concatenating them all together
    /// - If rendering to a string fails because the bytes are not valid UTF-8,
    ///   concatenate into a bytes object instead
    ///
    /// Return an error iff any chunk failed to render. This will never fail on
    /// output conversion because it can always fall back to returning raw
    /// bytes.
    pub async fn render_value<Ctx: Context>(
        &self,
        context: &Ctx,
    ) -> Result<Value, RenderError> {
        let mut chunks = self.render_chunks(context).await;

        // If we have a single dynamic chunk, return its value directly instead
        // of stringifying
        if let &[RenderedChunk::Rendered(_)] = chunks.as_slice() {
            let Some(RenderedChunk::Rendered(value)) = chunks.pop() else {
                // Checked pattern above
                unreachable!()
            };
            return Ok(value);
        }

        // Stitch together into bytes. Attempt to convert that UTF-8, but if
        // that fails fall back to just returning the bytes
        let bytes = chunks_to_bytes(chunks)?;
        match String::from_utf8(bytes.into()) {
            Ok(s) => Ok(Value::String(s)),
            Err(error) => Ok(Value::Bytes(error.into_bytes().into())),
        }
    }

    /// Render the template using values from the given context. If any chunk
    /// failed to render, return an error. The output is returned as bytes,
    /// meaning it can safely render to non-UTF-8 content. Use
    /// [Self::render_string] if you want the bytes converted to a string.
    pub async fn render_bytes<Ctx: Context>(
        &self,
        context: &Ctx,
    ) -> Result<Bytes, RenderError> {
        let chunks = self.render_chunks(context).await;
        chunks_to_bytes(chunks)
    }

    /// Render the template using values from the given context. If any chunk
    /// failed to render, return an error. The output will be converted from raw
    /// bytes to UTF-8. If it is not valid UTF-8, return an error.
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
    Boolean(bool),
    Integer(i64),
    Float(f64),
    String(String),
    #[from(skip)] // We use a generic impl instead
    Array(Vec<Self>),
    Object(IndexMap<String, Self>),
    // Put this at the end so int arrays deserialize as Array instead of Bytes
    Bytes(Bytes),
}

impl Value {
    /// Convert this value to a boolean, according to its truthiness.
    /// Truthiness/falsiness is defined for each type as:
    /// - `null` - `false`
    /// - `bool` - Own value
    /// - `integer` - `false` if zero
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
            Self::Boolean(b) => *b,
            Self::Integer(i) => *i != 0,
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
    pub fn try_into_string(self) -> Result<String, WithValue<ValueError>> {
        match self {
            Self::Null => Ok(NULL.into()),
            Self::Boolean(false) => Ok(FALSE.into()),
            Self::Boolean(true) => Ok(TRUE.into()),
            Self::Integer(i) => Ok(i.to_string()),
            Self::Float(f) => Ok(f.to_string()),
            Self::String(s) => Ok(s),
            Self::Bytes(bytes) => String::from_utf8(bytes.into())
                // We moved the value to convert it, so we have to reconstruct
                // it for the error
                .map_err(|error| {
                    WithValue::new(
                        Self::Bytes(error.as_bytes().to_owned().into()),
                        error.utf8_error(),
                    )
                }),
            // Use the display impl
            Self::Array(_) | Self::Object(_) => Ok(self.to_string()),
        }
    }

    /// Convert this value to a byte string. Bytes values are returned as is.
    /// Anything else is converted to a string first, then encoded as UTF-8.
    pub fn into_bytes(self) -> Bytes {
        match self {
            Self::Null => NULL.into(),
            Self::Boolean(false) => FALSE.into(),
            Self::Boolean(true) => TRUE.into(),
            Self::Integer(i) => i.to_string().into(),
            Self::Float(f) => f.to_string().into(),
            Self::String(s) => s.into(),
            Self::Bytes(bytes) => bytes,
            // Use the display impl
            Self::Array(_) | Self::Object(_) => self.to_string().into(),
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
            Literal::Boolean(b) => Value::Boolean(*b),
            Literal::Integer(i) => Value::Integer(*i),
            Literal::Float(f) => Value::Float(*f),
            Literal::String(s) => Value::String(s.clone()),
            Literal::Bytes(bytes) => Value::Bytes(bytes.clone()),
        }
    }
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Self::String(value.into())
    }
}

impl<T> From<Vec<T>> for Value
where
    Value: From<T>,
{
    fn from(value: Vec<T>) -> Self {
        Self::Array(value.into_iter().map(Self::from).collect())
    }
}

impl<K, V> From<Vec<(K, V)>> for Value
where
    String: From<K>,
    Value: From<V>,
{
    fn from(value: Vec<(K, V)>) -> Self {
        Self::Object(
            value
                .into_iter()
                .map(|(key, value)| (key.into(), value.into()))
                .collect(),
        )
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
    context: &'ctx Ctx,
    /// Position arguments. This queue will be drained from the front as
    /// arguments are converted, and additional arguments not accepted by the
    /// function will trigger an error.
    position: VecDeque<Value>,
    /// Number of arguments that have been popped off so far. Used to provide
    /// better error messages
    num_popped: usize,
    /// Keyword arguments. All keyword arguments are optional. Ordering has no
    /// impact on semantics, but we use an `IndexMap` so the order in error
    /// messages will match what the user passed.
    keyword: IndexMap<String, Value>,
}

impl<'ctx, Ctx> Arguments<'ctx, Ctx> {
    pub fn new(
        context: &'ctx Ctx,
        position: VecDeque<Value>,
        keyword: IndexMap<String, Value>,
    ) -> Self {
        Self {
            context,
            position,
            num_popped: 0,
            keyword,
        }
    }

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
            .ok_or(RenderError::TooFewArguments)?;
        let arg_index = self.num_popped;
        self.num_popped += 1;
        T::try_from_value(value).map_err(|error| {
            RenderError::Value(error.error).context(
                RenderErrorContext::ArgumentConvert {
                    argument: arg_index.to_string(),
                    value: error.value,
                },
            )
        })
    }

    /// Remove a keyword argument from the argument set, converting it to type
    /// `T` using its [TryFromValue] implementation. Return an error if the
    /// keyword argument does not exist or the conversion fails.
    pub fn pop_keyword<T: Default + TryFromValue>(
        &mut self,
        name: &str,
    ) -> Result<T, RenderError> {
        match self.keyword.shift_remove(name) {
            Some(value) => T::try_from_value(value).map_err(|error| {
                RenderError::Value(error.error).context(
                    RenderErrorContext::ArgumentConvert {
                        argument: name.to_owned(),
                        value: error.value,
                    },
                )
            }),
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
    fn try_from_value(value: Value) -> Result<Self, WithValue<ValueError>>;
}

impl TryFromValue for Value {
    fn try_from_value(value: Value) -> Result<Self, WithValue<ValueError>> {
        Ok(value)
    }
}

impl TryFromValue for bool {
    fn try_from_value(value: Value) -> Result<Self, WithValue<ValueError>> {
        Ok(value.to_bool())
    }
}

impl TryFromValue for f64 {
    fn try_from_value(value: Value) -> Result<Self, WithValue<ValueError>> {
        match value {
            Value::Float(f) => Ok(f),
            _ => Err(WithValue::new(
                value,
                ValueError::Type {
                    expected: Expected::Float,
                },
            )),
        }
    }
}

impl TryFromValue for i64 {
    fn try_from_value(value: Value) -> Result<Self, WithValue<ValueError>> {
        match value {
            Value::Integer(i) => Ok(i),
            _ => Err(WithValue::new(
                value,
                ValueError::Type {
                    expected: Expected::Integer,
                },
            )),
        }
    }
}

impl TryFromValue for String {
    fn try_from_value(value: Value) -> Result<Self, WithValue<ValueError>> {
        // This will succeed for anything other than invalid UTF-8 bytes
        value.try_into_string()
    }
}

impl TryFromValue for Bytes {
    fn try_from_value(value: Value) -> Result<Self, WithValue<ValueError>> {
        Ok(value.into_bytes())
    }
}

impl<T> TryFromValue for Option<T>
where
    T: TryFromValue,
{
    fn try_from_value(value: Value) -> Result<Self, WithValue<ValueError>> {
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
    fn try_from_value(value: Value) -> Result<Self, WithValue<ValueError>> {
        if let Value::Array(array) = value {
            array.into_iter().map(T::try_from_value).collect()
        } else {
            Err(WithValue::new(
                value,
                ValueError::Type {
                    expected: Expected::Array,
                },
            ))
        }
    }
}

impl From<serde_json::Value> for Value {
    fn from(value: serde_json::Value) -> Self {
        Self::from_json(value)
    }
}

/// Convert a template value to JSON. If the value is bytes, this will
/// deserialize it as JSON, otherwise it will convert directly. This allows us
/// to parse response bodies as JSON while accepting anything else as a native
/// JSON value
impl TryFromValue for serde_json::Value {
    fn try_from_value(value: Value) -> Result<Self, WithValue<ValueError>> {
        match value {
            Value::Null => Ok(serde_json::Value::Null),
            Value::Boolean(b) => Ok(b.into()),
            Value::Integer(i) => Ok(i.into()),
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
            Value::Bytes(_) => {
                // Bytes are probably a string. If it's not UTF-8 there's no way
                // to make JSON from it
                value.try_into_string().map(serde_json::Value::String)
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
            ) -> Result<Self, $crate::WithValue<$crate::ValueError>> {
                let s = String::try_from_value(value)?;
                s.parse().map_err(|error| {
                    $crate::WithValue::new(
                        s.into(),
                        $crate::ValueError::other(error),
                    )
                })
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

/// Concatenate rendered chunks into bytes. If any chunk is an error, return an
/// error
fn chunks_to_bytes(chunks: Vec<RenderedChunk>) -> Result<Bytes, RenderError> {
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
                RenderedChunk::Rendered(value) => {
                    acc.extend(value.into_bytes());
                }
                RenderedChunk::Error(error) => return Err(error),
            }
            Ok(acc)
        })
        .map(Bytes::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::indexmap;
    use rstest::rstest;
    use slumber_util::assert_err;

    /// Test simple expression rendering
    #[rstest]
    #[case::object(
        "{{ {'a': 1, 1: 2, ['a',1]: ['b',2]} }}",
        vec![
            ("a", Value::from(1)),
            ("1", 2.into()),
            // Note the whitespace in the key: it was parsed and restringified
            ("['a', 1]", vec![Value::from("b"), 2.into()].into()),
        ].into(),
    )]
    #[case::object_dupe_key(
        // Latest entry takes precedence
        "{{ {'Mike': 1, name: 2, 10: 3, '10': 4} }}",
        vec![("Mike", 2), ("10", 4)].into(),
    )]
    #[tokio::test]
    async fn test_expression(
        #[case] template: Template,
        #[case] expected: Value,
    ) {
        assert_eq!(
            template.render_value(&TestContext).await.unwrap(),
            expected
        );
    }

    /// Render to a value. Templates with a single dynamic chunk are allowed to
    /// produce non-string values. This is specifically testing the behavior
    /// of [Template::render_value], rather than expression evaluation.
    #[rstest]
    #[case::unpack("{{ array }}", vec!["a", "b", "c"].into())]
    #[case::string("my name is {{ name }}", "my name is Mike".into())]
    #[case::bytes(
        "my name is {{ invalid_utf8 }}",
        Value::Bytes(b"my name is \xc3\x28".as_slice().into(),
    ))]
    #[tokio::test]
    async fn test_render_value(
        #[case] template: Template,
        #[case] expected: Value,
    ) {
        assert_eq!(
            template.render_value(&TestContext).await.unwrap(),
            expected
        );
    }

    /// Convert JSON values to template values
    #[rstest]
    #[case::null(serde_json::Value::Null, Value::Null)]
    #[case::bool_true(serde_json::Value::Bool(true), Value::Boolean(true))]
    #[case::bool_false(serde_json::Value::Bool(false), Value::Boolean(false))]
    #[case::number_positive_int(serde_json::json!(42), Value::Integer(42))]
    #[case::number_negative_int(serde_json::json!(-17), Value::Integer(-17))]
    #[case::number_zero(serde_json::json!(0), Value::Integer(0))]
    #[case::number_float(serde_json::json!(1.23), Value::Float(1.23))]
    #[case::number_negative_float(serde_json::json!(-2.5), Value::Float(-2.5))]
    #[case::number_zero_float(serde_json::json!(0.0), Value::Float(0.0))]
    #[case::string_empty(serde_json::json!(""), "".into())]
    #[case::string_simple(serde_json::json!("hello"), "hello".into())]
    #[case::string_with_spaces(serde_json::json!("hello world"), "hello world".into())]
    #[case::string_with_unicode(serde_json::json!("héllo 🌍"), "héllo 🌍".into())]
    #[case::string_with_escapes(serde_json::json!("line1\nline2\ttab"), "line1\nline2\ttab".into())]
    #[case::array(
        serde_json::json!([null, true, 42, "hello"]),
        Value::Array(vec![
            Value::Null,
            Value::Boolean(true),
            Value::Integer(42),
            "hello".into(),
        ])
    )]
    // Array of numbers should *not* be interpreted as bytes
    #[case::array_numbers(serde_json::json!([1, 2, 3]), vec![1, 2, 3].into())]
    #[case::array_nested(
        serde_json::json!([[1, 2], [3, 4]]),
        vec![Value::from(vec![1, 2]), Value::from(vec![3, 4])].into()
    )]
    #[case::object(
        serde_json::json!({"name": "John", "age": 30, "active": true}),
        Value::Object(indexmap! {
            "name".into() => "John".into(),
            "age".into() => Value::Integer(30),
            "active".into() => Value::Boolean(true),
        })
    )]
    #[case::object_nested(
        serde_json::json!({"user": {"name": "Alice", "scores": [95, 87]}}),
        Value::Object(indexmap! {
            "user".into() => Value::Object(indexmap! {
                "name".into() => "Alice".into(),
                "scores".into() =>
                    Value::Array(vec![Value::Integer(95), Value::Integer(87)]),
            })
        })
    )]
    fn test_from_json(
        #[case] json: serde_json::Value,
        #[case] expected: Value,
    ) {
        let actual = Value::from_json(json);
        assert_eq!(actual, expected);
    }

    #[rstest]
    #[case::one_arg("{{ 1 | identity() }}", "1")]
    #[case::multiple_args("{{ 'cd' | concat('ab') }}", "abcd")]
    // Piped value is the last positional arg, before kwargs
    #[case::kwargs("{{ 'cd' | concat('ab', reverse=true) }}", "dcba")]
    #[tokio::test]
    async fn test_pipe(#[case] template: Template, #[case] expected: &str) {
        assert_eq!(
            template.render_string(&TestContext).await.unwrap(),
            expected
        );
    }

    /// Test error context on a variety of error cases in function calls
    #[rstest]
    #[case::unknown_function("{{ fake() }}", "fake(): Unknown function")]
    #[case::extra_arg(
        "{{ identity('a', 'b') }}",
        "identity(): Extra arguments 'b'"
    )]
    #[case::missing_arg("{{ add(1) }}", "add(): Not enough arguments")]
    #[case::arg_render(
        // Argument fails to render
        "{{ add(f(), 2) }}",
        "add(): argument 0=f(): f(): Unknown function"
    )]
    #[case::arg_convert(
        // Argument renders but doesn't convert to what the func wants
        "{{ add(1, 'b') }}",
        "add(): argument 1='b': Expected integer"
    )]
    #[tokio::test]
    async fn test_function_error(
        #[case] template: Template,
        #[case] expected_error: &str,
    ) {
        assert_err!(
            // Use anyhow to get the error message to include the whole chain
            template
                .render_string(&TestContext)
                .await
                .map_err(anyhow::Error::from),
            expected_error
        );
    }

    struct TestContext;

    impl Context for TestContext {
        async fn get(
            &self,
            identifier: &Identifier,
        ) -> Result<Value, RenderError> {
            match identifier.as_str() {
                "name" => Ok("Mike".into()),
                "array" => Ok(vec!["a", "b", "c"].into()),
                "invalid_utf8" => {
                    Ok(Value::Bytes(b"\xc3\x28".as_slice().into()))
                }
                _ => Err(RenderError::FieldUnknown {
                    field: identifier.clone(),
                }),
            }
        }

        async fn call(
            &self,
            function_name: &Identifier,
            mut arguments: Arguments<'_, Self>,
        ) -> Result<Value, RenderError> {
            match function_name.as_str() {
                "identity" => {
                    let value: Value = arguments.pop_position()?;
                    arguments.ensure_consumed()?;
                    Ok(value)
                }
                "add" => {
                    let a: i64 = arguments.pop_position()?;
                    let b: i64 = arguments.pop_position()?;
                    arguments.ensure_consumed()?;
                    Ok((a + b).into())
                }
                "concat" => {
                    let mut a: String = arguments.pop_position()?;
                    let b: String = arguments.pop_position()?;
                    let reverse: bool = arguments.pop_keyword("reverse")?;
                    arguments.ensure_consumed()?;
                    a.push_str(&b);
                    if reverse {
                        Ok(a.chars().rev().collect::<String>().into())
                    } else {
                        Ok(a.into())
                    }
                }
                _ => Err(RenderError::FunctionUnknown),
            }
        }
    }
}
