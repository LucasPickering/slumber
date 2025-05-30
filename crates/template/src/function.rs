//! Template function framework, including value conversions for function
//! arguments and outputs

use crate::{TemplateError, Value};
use serde::{Deserialize, de::IntoDeserializer};
use std::{
    collections::{HashMap, VecDeque},
    fmt::Debug,
};

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
    pub fn pop_position<T: TryFromValue>(
        &mut self,
    ) -> Result<T, TemplateError> {
        let value = self
            .position
            .pop_front()
            .ok_or(TemplateError::NotEnoughArguments)?;
        T::try_from_value(value)
    }

    /// Pop the next positional argument off the front of the queue and convert
    /// it to type `T` using its [Deserialize] implementation. Return an error
    /// if there are no positional arguments left or the conversion fails.
    pub fn pop_position_serde<'de, T: Deserialize<'de>>(
        &mut self,
    ) -> Result<T, TemplateError> {
        let value = self
            .position
            .pop_front()
            .ok_or(TemplateError::NotEnoughArguments)?;
        T::deserialize(value.into_deserializer())
    }

    /// Remove a keyword argument from the argument set, converting it to type
    /// `T` using its [TryFromValue] implementation. Return an error if the
    /// keyword argument does not exist or the conversion fails.
    pub fn pop_keyword<T: Default + TryFromValue>(
        &mut self,
        name: &str,
    ) -> Result<T, TemplateError> {
        match self.keyword.remove(name) {
            Some(value) => T::try_from_value(value),
            // Kwarg not provided - use the default value
            None => Ok(T::default()),
        }
    }

    /// Remove a keyword argument from the argument set, converting it to type
    /// `T` using its [Deserialize] implementation. Return an error if the
    /// keyword argument does not exist or the conversion fails.
    pub fn pop_keyword_serde<'de, T: Default + Deserialize<'de>>(
        &mut self,
        name: &str,
    ) -> Result<T, TemplateError> {
        match self.keyword.remove(name) {
            Some(value) => T::deserialize(value.into_deserializer()),
            // Kwarg not provided - use the default value
            None => Ok(T::default()),
        }
    }

    /// Ensure that all positional and keyword arguments have been consumed.
    /// Return an error if any arguments were passed by the user but not
    /// consumed by the function implementation.
    pub fn ensure_consumed(self) -> Result<(), TemplateError> {
        if self.position.is_empty() && self.keyword.is_empty() {
            Ok(())
        } else {
            Err(TemplateError::TooManyArguments {
                position: self.position.into(),
                keyword: self.keyword,
            })
        }
    }
}

/// TODO
pub trait TryFromValue: Sized {
    /// TODO
    fn try_from_value(value: Value) -> Result<Self, TemplateError>;
}

impl TryFromValue for bool {
    fn try_from_value(value: Value) -> Result<Self, TemplateError> {
        Ok(value.to_bool())
    }
}

impl TryFromValue for String {
    fn try_from_value(value: Value) -> Result<Self, TemplateError> {
        // This will succeed for anything other than invalid UTF-8 bytes
        value.try_into_string()
    }
}

impl<T> TryFromValue for Option<T>
where
    T: TryFromValue,
{
    fn try_from_value(value: Value) -> Result<Self, TemplateError> {
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
    fn try_from_value(value: Value) -> Result<Self, TemplateError> {
        if let Value::Array(array) = value {
            array.into_iter().map(T::try_from_value).collect()
        } else {
            Err(TemplateError::Type {
                expected: "array",
                actual: value,
            })
        }
    }
}

/// Trait representing a value that can be converted into `Result<Value,
/// TemplateError>`. This conversion is used to make all function definitions
/// provide uniform output. The return type of any registered [Function] must
/// implement this trait.
pub trait FunctionOutput {
    fn into_result(self) -> Result<Value, TemplateError>;
}

impl<T> FunctionOutput for T
where
    Value: From<T>,
{
    fn into_result(self) -> Result<Value, TemplateError> {
        Ok(self.into())
    }
}

impl<T, E> FunctionOutput for Result<T, E>
where
    T: Into<Value> + Send + Sync,
    E: Into<TemplateError> + Send + Sync,
{
    fn into_result(self) -> Result<Value, TemplateError> {
        self.map(T::into).map_err(E::into)
    }
}

impl<T: FunctionOutput> FunctionOutput for Option<T> {
    fn into_result(self) -> Result<Value, TemplateError> {
        self.map(T::into_result).unwrap_or(Ok(Value::Null))
    }
}
