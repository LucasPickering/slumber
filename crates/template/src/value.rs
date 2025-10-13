//! Template runtime values

use crate::{
    Expected, Literal, RenderError, RenderedOutput, ValueError, WithValue,
    error::RenderErrorContext,
    parse::{FALSE, NULL, TRUE},
};
use bytes::{Bytes, BytesMut};
use derive_more::{Display, From};
use futures::{TryStreamExt, stream::BoxStream};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::{collections::VecDeque, fmt::Debug, path::PathBuf};

/// A runtime template value. This very similar to a JSON value, except:
/// - Numbers do not support arbitrary size
/// - Bytes are supported
#[derive(Clone, Debug, Default, From, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    #[default]
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

// Convert from byte literals
impl<const N: usize> From<&'static [u8; N]> for Value {
    fn from(value: &'static [u8; N]) -> Self {
        Self::Bytes(value.as_slice().into())
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

impl From<serde_json::Value> for Value {
    fn from(value: serde_json::Value) -> Self {
        Self::from_json(value)
    }
}

/// A source of a template value. This can be a concrete [Value] or a streamable
/// source such as a file. This is used widely within rendering because it's a
/// superset of all values. Not all renders accept streams as results though,
/// so it's a separate type rather than a variant on [Value]. To convert a
/// stream into a value, call [Self::resolve].
#[derive(derive_more::Debug)]
pub enum LazyValue {
    /// A pre-resolved value
    Value(Value),
    /// Stream data from a (potentially) large source such as a file
    Stream {
        /// Additional information about the source of the stream
        source: StreamSource,
        /// The stream of binary data
        #[debug(skip)]
        stream: BoxStream<'static, Result<Bytes, RenderError>>,
    },
    /// A template chunk that rendered a nested template with multiple chunks
    Nested(RenderedOutput),
}

impl LazyValue {
    /// Resolve this lazy value to a concrete [Value]. If it's already a value,
    /// just return it. If it's a stream it will be awaited and collected
    /// into bytes. If it's nested chunks, collect them into a single value.
    pub async fn resolve(self) -> Result<Value, RenderError> {
        match self {
            Self::Value(value) => Ok(value),
            Self::Stream { stream, .. } => stream
                .try_collect::<BytesMut>()
                .await
                .map(|bytes| Value::Bytes(bytes.into())),
            // Box needed for recursion
            Self::Nested(output) => Box::pin(output.try_collect_value()).await,
        }
    }
}

impl<T: Into<Value>> From<T> for LazyValue {
    fn from(value: T) -> Self {
        Self::Value(value.into())
    }
}

/// Metadata about the source of a [Stream](LazyValue::Stream). This helps
/// consumers present the stream to the user, e.g. in a template preview
#[derive(Clone, Debug, Display, PartialEq)]
pub enum StreamSource {
    /// Stream from a subprocess
    #[display("command `{}`", command.join(" "))]
    Command {
        /// Program + 0 or more arguments
        command: Vec<String>,
    },
    /// Data is being streamed from a file
    #[display("file {}", path.display())]
    File {
        /// **Absolute** path to the file
        path: PathBuf,
    },
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
/// [String], then using `T`'s `FromStr` implementation to convert to `T`.
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

/// Arguments passed to a function call
///
/// This container holds all the data a template function may need to construct
/// its own arguments. All given positional and keyword arguments are expected
/// to be used, and [ensure_consumed](Self::ensure_consumed) should be called
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

    /// Push a piped argument onto the back of the positional argument list
    pub(crate) fn push_piped(&mut self, argument: Value) {
        self.position.push_back(argument);
    }
}

/// Convert any value into `Result<Value, RenderError>`
///
/// This is used for converting function outputs back to template values.
pub trait FunctionOutput {
    fn into_result(self) -> Result<LazyValue, RenderError>;
}

impl<T: Into<LazyValue>> FunctionOutput for T {
    fn into_result(self) -> Result<LazyValue, RenderError> {
        Ok(self.into())
    }
}

impl<T, E> FunctionOutput for Result<T, E>
where
    T: Into<LazyValue>,
    E: Into<RenderError>,
{
    fn into_result(self) -> Result<LazyValue, RenderError> {
        self.map(T::into).map_err(E::into)
    }
}

impl<T: FunctionOutput> FunctionOutput for Option<T> {
    fn into_result(self) -> Result<LazyValue, RenderError> {
        self.map(T::into_result).unwrap_or(Ok(Value::Null.into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RenderedChunk;
    use futures::{StreamExt, future, stream};
    use rstest::rstest;
    use slumber_util::assert_result;

    #[rstest]
    #[case::value(LazyValue::Value("test".into()), Ok("test".into()))]
    #[case::stream(
        stream(Ok("test".into())),
        Ok(b"test".into()),
    )]
    #[case::stream_error(
        stream(Err(RenderError::FunctionUnknown)),
        Err("Unknown function")
    )]
    #[case::nested(
        LazyValue::Nested(RenderedOutput(vec![
            RenderedChunk::Rendered(LazyValue::Value("test1".into())),
            RenderedChunk::Raw(" ".into()),
            RenderedChunk::Rendered(stream(Ok("test2".into()))),
        ])),
        Ok("test1 test2".into()),
    )]
    #[case::nested_error(
        LazyValue::Nested(RenderedOutput(vec![
            RenderedChunk::Rendered(LazyValue::Value("test1".into())),
            RenderedChunk::Raw(" ".into()),
            RenderedChunk::Rendered(stream(Err(RenderError::FunctionUnknown))),
        ])),
        Err("Unknown function"),
    )]
    #[tokio::test]
    async fn test_lazy_resolve(
        #[case] lazy: LazyValue,
        #[case] expected: Result<Value, &str>,
    ) {
        assert_result(lazy.resolve().await, expected);
    }

    fn stream(result: Result<Bytes, RenderError>) -> LazyValue {
        LazyValue::Stream {
            stream: stream::once(future::ready(result)).boxed(),
            source: StreamSource::File {
                path: "bogus".into(),
            },
        }
    }
}
