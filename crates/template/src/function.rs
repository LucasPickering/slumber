//! Template function and filter framework

use crate::{TemplateError, Value};
use serde::{
    Deserialize,
    de::{IntoDeserializer, value::MapDeserializer},
};
use std::{collections::VecDeque, fmt::Debug, mem};

/// Arguments passed to a function call
#[derive(Debug)]
pub struct Arguments<'ctx, Ctx> {
    /// TODO
    pub context: &'ctx Ctx,
    /// Position arguments. This queue will be drained from the front as
    /// arguments are converted, and additional arguments not accepted by the
    /// function will trigger an error.
    pub position: VecDeque<Value>,
    /// Keyword arguments. These will be converted wholesale as a single map,
    /// as there's no Rust support for kwargs. All keyword arguments are
    /// optional.
    pub keyword: Vec<(String, Value)>,
}

impl<'ctx, Ctx> Arguments<'ctx, Ctx> {
    /// Pop the next positional argument off the front of the queue. Return an
    /// error if there are no positional arguments left
    fn pop_position(&mut self) -> Result<Value, TemplateError> {
        self.position
            .pop_front()
            .ok_or(TemplateError::NotEnoughArguments)
    }

    /// Move all keyword arguments out of this struct
    fn take_keyword(&mut self) -> Vec<(String, Value)> {
        mem::take(&mut self.keyword)
    }

    /// TODO rename
    pub fn try_into<T>(self) -> Result<T, TemplateError>
    where
        T: FromArguments<'ctx, Ctx>,
    {
        T::from_arguments(self)
    }
}

/// TODO
pub trait FromArguments<'ctx, Ctx>: Sized {
    fn from_arguments(
        arguments: Arguments<'ctx, Ctx>,
    ) -> Result<Self, TemplateError>;
}

/// Implement [Function] for a set of argument types.
///
/// Example: `tuple_impls! { T0, T1 }` will implement [Function] on
/// `Fn(T0, T1) -> Out`
macro_rules! tuple_impls {
    ($($arg_type:ident)*) => {
        #[allow(clippy::allow_annotations)]
        impl<'ctx, Ctx, $($arg_type,)*> FromArguments<'ctx, Ctx>
            for ($($arg_type,)*)
        where
            $($arg_type: FunctionArg<'ctx, Ctx> + Send + Sync,)*
        {
            #[allow(non_snake_case)]
            #[allow(unused)]
            fn from_arguments(
                mut arguments: Arguments<'ctx, Ctx>,
            ) -> Result<Self, TemplateError> {
                // Unpack args, convert them, and pass them to the function. We
                // convert args left-to-right so that each positional arg can
                // pop off the front of the argument queue. We can reuse the
                // type name as the var name to avoid complicated
                // transformations or recursion
                // TODO error for lingering args in the queue
                $(let $arg_type = $arg_type::from_arguments(&mut arguments)?;)*
                Ok(($($arg_type,)*))
            }
        }
    };
}

// Accept functions of 0-5 arguments
tuple_impls! {}
tuple_impls! { T0 }
tuple_impls! { T0 T1 }
tuple_impls! { T0 T1 T2 }
tuple_impls! { T0 T1 T2 T3 }
tuple_impls! { T0 T1 T2 T3 T4 }

/// TODO
///
/// ## Type Params
///
/// - `'ctx` is the lifetime of the given references to the engine and context.
/// - `Ctx` is the template context type
trait FunctionArg<'ctx, Ctx>: Sized {
    /// Get this value from the argument object and convert it. For most types
    /// this will grab the next positional arguments, but this also exposes
    /// the context and keyword arguments which can be used. If an argument
    /// is converted, it should be removed.
    fn from_arguments(
        arguments: &mut Arguments<'ctx, Ctx>,
    ) -> Result<Self, TemplateError>;
}

/// Get the template context as a function arg
impl<'ctx, Ctx: 'ctx> FunctionArg<'ctx, Ctx> for &'ctx Ctx {
    fn from_arguments(
        arguments: &mut Arguments<'ctx, Ctx>,
    ) -> Result<Self, TemplateError> {
        Ok(arguments.context)
    }
}

impl<'ctx, Ctx> FunctionArg<'ctx, Ctx> for String {
    fn from_arguments(
        arguments: &mut Arguments<'ctx, Ctx>,
    ) -> Result<Self, TemplateError> {
        let value = arguments.pop_position()?;
        if let Value::String(s) = value {
            Ok(s)
        } else {
            todo!("error? convert to string?")
        }
    }
}

impl<'ctx, Ctx, T> FunctionArg<'ctx, Ctx> for Vec<T>
where
    T: FunctionArg<'ctx, Ctx>,
{
    fn from_arguments(
        arguments: &mut Arguments<'ctx, Ctx>,
    ) -> Result<Self, TemplateError> {
        let value = arguments.pop_position()?;
        if let Value::Array(array) = value {
            Ok(array
                .into_iter()
                .map(T::from_arguments)
                .collect::<Result<Vec<_>, _>>()?)
        } else {
            todo!("error")
        }
    }
}

/// Convert a [Value] to a function argument using the argument type's
/// [Deserialize] implementation.
pub struct ViaSerde<T>(pub T);

impl<'ctx, 'de, Ctx, T> FunctionArg<'ctx, Ctx> for ViaSerde<T>
where
    T: Deserialize<'de>,
{
    fn from_arguments(
        arguments: &mut Arguments<'ctx, Ctx>,
    ) -> Result<Self, TemplateError> {
        let value = arguments.pop_position()?;
        T::deserialize(value.into_deserializer()).map(Self)
    }
}

/// Wrapper for a keyword argument struct, which will be deserialized from a
/// a mapping of keywords. Kwargs should only be used for additional options to
/// a function that are not required. As such, the struct should include
/// `#[serde(default)]` so it can be deserialized when keyword args are missing.
/// It should also include `#[serde(deny_unknown_fields)]` to prevent passing
/// unrecognized keyword arguments.
pub struct Kwargs<T>(pub T);

impl<'ctx, 'de, Ctx, T> FunctionArg<'ctx, Ctx> for Kwargs<T>
where
    T: Deserialize<'de>,
{
    fn from_arguments(
        arguments: &mut Arguments<'ctx, Ctx>,
    ) -> Result<Self, TemplateError> {
        // Use a generic deserializer to convert the kwargs as a mapping
        let deserializer =
            MapDeserializer::new(arguments.take_keyword().into_iter());
        T::deserialize(deserializer).map(Self)
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
