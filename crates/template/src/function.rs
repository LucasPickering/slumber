//! Template function and filter framework

use crate::{TemplateEngine, TemplateError, Value};
use serde::{
    Deserialize,
    de::{IntoDeserializer, value::MapDeserializer},
};
use std::{
    collections::VecDeque,
    fmt::{self, Debug},
    mem,
};

/// TODO
pub(crate) struct BoxedFunction<Ctx>(
    #[expect(clippy::type_complexity)]
    Box<
        dyn for<'a> Fn(Arguments<'a, Ctx>) -> Result<Value, TemplateError>
            + Send
            + Sync,
    >,
);

impl<Ctx> BoxedFunction<Ctx> {
    /// Wrap the given function, whose type is statically known, into a dynamic
    /// object so it can be stored in the function map. This will provide
    /// argument and output conversion to make the function type uniform.
    pub(crate) fn new<F, Args, Out>(function: F) -> Self
    where
        F: Function<Ctx, Args, Out>,
    {
        let wrap =
            move |arguments: Arguments<'_, Ctx>| function.invoke(arguments);
        Self(Box::new(wrap))
    }

    /// Call this function
    pub(crate) fn invoke(
        &self,
        arguments: Arguments<'_, Ctx>,
    ) -> Result<Value, TemplateError> {
        (self.0)(arguments)
    }
}

impl<Ctx> Debug for BoxedFunction<Ctx> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Print the pointer to the function as something identifying
        write!(f, "BoxedFunction({:p})", self.0)
    }
}

/// Arguments passed to a function call
#[derive(Debug)]
pub struct Arguments<'a, Ctx> {
    /// TODO
    pub engine: &'a TemplateEngine<Ctx>,
    /// TODO
    pub context: &'a Ctx,
    /// Position arguments. This queue will be drained from the front as
    /// arguments are converted, and additional arguments not accepted by the
    /// function will trigger an error.
    pub position: VecDeque<Value>,
    /// Keyword arguments. These will be converted wholesale as a single map,
    /// as there's no Rust support for kwargs. All keyword arguments are
    /// optional.
    pub keyword: Vec<(String, Value)>,
}

impl<Ctx> Arguments<'_, Ctx> {
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
}

/// TODO
pub trait Function<Ctx, Args, Out>: 'static + Send + Sync {
    fn invoke<'a>(
        &self,
        arguments: Arguments<'a, Ctx>,
    ) -> Result<Value, TemplateError>;
}

/// Implement [Function] for a set of argument types.
///
/// Example: `tuple_impls! { T0, T1 }` will implement [Function] on
/// `Fn(T0, T1) -> Out`
macro_rules! tuple_impls {
    ($($arg_type:ident)*) => {
        #[allow(clippy::allow_annotations)]
        impl<Ctx, F, $($arg_type,)* Out> Function<Ctx, ($($arg_type,)*), Out>
            for F
        where
            $($arg_type: for<'a> FunctionArg<'a, Ctx> + Send + Sync,)*
            Out: FunctionOutput,
            F: 'static + Fn($(<$arg_type as FunctionArg<'_, Ctx>>::Output),*) -> Out + Send + Sync,
        {
            #[allow(non_snake_case)]
            #[allow(unused)]
            fn invoke<'a>(
                &self,
                mut arguments: Arguments<'a, Ctx>,
            ) -> Result<Value, TemplateError> {
                // Unpack args, convert them, and pass them to the function
                // TODO error for lingering args in the queue
                // Convert each argument and pack them into a tuple. We convert
                // args left-to-right so that each positional arg can pop off
                // the front of the argument queue
                // TODO update comment
                // We can reuse the type name as the var name to avoid
                // complicated transformations or recursion
                $(let $arg_type = $arg_type::from_arguments(&mut arguments)?;)*
                (self)($($arg_type,)*).into_result()
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
pub trait FunctionArg<'a, Ctx>: Sized {
    /// The output type of the conversion, i.e. the type of the resulting
    /// argument. Typically this is just `Self`. The associated type is needed
    /// to declare the lifetimes correctly for the implementations on
    /// [TemplateEngine] and the template context. Those return references  with
    /// the `'a` lifetime, and apparently just implementing the trait on `&'a T`
    /// doesn't work. Somewhere the borrow checker's wires get crossed on the
    /// lifetime and you end up with an implementation that isn't general
    /// enough.
    type Output;

    fn from_arguments(
        arguments: &mut Arguments<'a, Ctx>,
    ) -> Result<Self::Output, TemplateError>;
}

/// Get the template engine as a function arg
impl<'a, Ctx: 'a> FunctionArg<'a, Ctx> for &TemplateEngine<Ctx> {
    type Output = &'a TemplateEngine<Ctx>;

    fn from_arguments(
        arguments: &mut Arguments<'a, Ctx>,
    ) -> Result<Self::Output, TemplateError> {
        Ok(arguments.engine)
    }
}

/// Get the template context as a function arg
impl<'a, Ctx: 'a> FunctionArg<'a, Ctx> for &Ctx {
    type Output = &'a Ctx;

    fn from_arguments(
        arguments: &mut Arguments<'a, Ctx>,
    ) -> Result<Self::Output, TemplateError> {
        Ok(arguments.context)
    }
}

impl<'a, Ctx> FunctionArg<'a, Ctx> for String {
    type Output = Self;

    fn from_arguments(
        arguments: &mut Arguments<'a, Ctx>,
    ) -> Result<Self::Output, TemplateError> {
        let value = arguments.pop_position()?;
        if let Value::String(s) = value {
            Ok(s)
        } else {
            todo!("error? convert to string?")
        }
    }
}

/// Convert a [Value] to a function argument using the argument type's
/// [Deserialize] implementation.
pub struct ViaSerde<T>(pub T);

impl<'a, 'de, Ctx, T> FunctionArg<'a, Ctx> for ViaSerde<T>
where
    T: Deserialize<'de>,
{
    type Output = Self;

    fn from_arguments(
        arguments: &mut Arguments<'a, Ctx>,
    ) -> Result<Self::Output, TemplateError> {
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

impl<'a, 'de, Ctx, T> FunctionArg<'a, Ctx> for Kwargs<T>
where
    T: Deserialize<'de>,
{
    type Output = Self;

    fn from_arguments(
        arguments: &mut Arguments<'a, Ctx>,
    ) -> Result<Self::Output, TemplateError> {
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

impl FunctionOutput for String {
    fn into_result(self) -> Result<Value, TemplateError> {
        Ok(Value::String(self))
    }
}

impl<T: FunctionOutput> FunctionOutput for Option<T> {
    fn into_result(self) -> Result<Value, TemplateError> {
        self.map(T::into_result).unwrap_or(Ok(Value::Null))
    }
}
