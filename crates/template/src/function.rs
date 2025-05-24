//! Template function and filter framework

use crate::{
    TemplateContext, TemplateEngine, TemplateError, Value, render::RenderResult,
};
use futures::{
    FutureExt,
    future::{self, BoxFuture},
};
use serde::{Deserialize, de::value::MapDeserializer};
use std::{
    fmt::{self, Debug},
    mem,
};

/// TODO
#[derive(Debug)]
pub enum Function<Ctx> {
    Filter(Filter<Ctx>),
    Producer(Producer<Ctx>),
}

impl<Ctx> Function<Ctx> {
    /// TODO
    pub(crate) fn as_filter(&self) -> Result<&Filter<Ctx>, TemplateError> {
        if let Self::Filter(filter) = self {
            Ok(filter)
        } else {
            Err(TemplateError::ExpectedFilter)
        }
    }

    /// TODO
    pub(crate) fn as_producer(&self) -> Result<&Producer<Ctx>, TemplateError> {
        if let Self::Producer(producer) = self {
            Ok(producer)
        } else {
            Err(TemplateError::ExpectedProducer)
        }
    }
}

/// Filters modify some input value, optionally based on some given arguments.
/// They exclusively live on the right side of a `|` operation. For example, in
/// `f() | trim()`, `f` is a producer and `trim` is a filter
pub struct Filter<Ctx>(
    #[expect(clippy::type_complexity)]
    Box<
        dyn for<'ctx> Fn(
                &'ctx TemplateEngine<Ctx>,
                &'ctx Ctx,
                Value, // Value to filter
                Arguments,
            ) -> BoxFuture<'ctx, RenderResult>
            + Send
            + Sync,
    >,
);

impl<Ctx> Filter<Ctx> {
    /// Invoke this filter, apply some operation to the given value
    pub(crate) async fn call(
        &self,
        engine: &TemplateEngine<Ctx>,
        context: &Ctx,
        value: Value,
        arguments: Arguments,
    ) -> RenderResult {
        (self.0)(engine, context, value, arguments).await
    }
}

impl<Ctx> Debug for Filter<Ctx> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Print the pointer to the function as something identifying
        write!(f, "Filter({:p})", self.0)
    }
}

/// TODO
pub struct Producer<Ctx>(
    #[expect(clippy::type_complexity)]
    Box<
        dyn for<'ctx> Fn(
                &'ctx TemplateEngine<Ctx>,
                &'ctx Ctx,
                Arguments,
            ) -> BoxFuture<'ctx, RenderResult>
            + Send
            + Sync,
    >,
);

impl<Ctx> Producer<Ctx> {
    /// Invoke this producer, creating a new value
    pub(crate) async fn call(
        &self,
        engine: &TemplateEngine<Ctx>,
        context: &Ctx,
        arguments: Arguments,
    ) -> RenderResult {
        (self.0)(engine, context, arguments).await
    }
}

impl<Ctx> Debug for Producer<Ctx> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Print the pointer to the function as something identifying
        write!(f, "Producer({:p})", self.0)
    }
}

/// Arguments passed to a function call
#[derive(Debug)]
pub struct Arguments {
    pub position: Vec<Value>,
    pub keyword: Vec<(String, Value)>,
}

/// TODO
pub trait IntoFilter<Ctx> {
    fn into_filter(self) -> Filter<Ctx>;
}

/// TODO
pub trait IntoProducer<Ctx, Args, Out> {
    fn into_producer(self) -> Producer<Ctx>;
}

impl<Ctx, F, Arg0, Out> IntoProducer<Ctx, (Arg0,), Out> for F
where
    F: 'static + Fn(Arg0) -> Out + Send + Sync,
    Ctx: TemplateContext,
    Arg0: for<'ctx> FunctionArg<'ctx, Ctx>,
    Out: FunctionOutput,
{
    fn into_producer(self) -> Producer<Ctx> {
        let f = move |engine: &TemplateEngine<Ctx>,
                      context: &Ctx,
                      mut arguments: Arguments|
              -> BoxFuture<'_, RenderResult> {
            let result = Arg0::from_arguments(engine, context, &mut arguments)
                .and_then(|arg0| (self)(arg0).into_result());
            future::ready(result).boxed()
        };
        Producer(Box::new(f))
    }
}

pub trait FunctionTodo<'ctx, Ctx, Args, Out>: Send + Sync + 'static
where
    Args: FunctionArgs<'ctx, Ctx>,
{
    fn invoke(&self, args: Args) -> BoxFuture<'ctx, Out>;
}

/// TODO
///
/// ## Type Params
///
/// - `'ctx` is the lifetime of the given references to the engine and context.
/// - `Ctx` is the template context type
pub trait FunctionArgs<'ctx, Ctx>: Sized {
    fn from_arguments(
        engine: &'ctx TemplateEngine<Ctx>,
        context: &'ctx Ctx,
        arguments: &mut Arguments,
    ) -> Result<Self, TemplateError>;
}

/// TODO
///
/// ## Type Params
///
/// - `'ctx` is the lifetime of the given references to the engine and context.
/// - `Ctx` is the template context type
pub trait FunctionArg<'ctx, Ctx>: Sized {
    fn from_arguments(
        engine: &'ctx TemplateEngine<Ctx>,
        context: &'ctx Ctx,
        arguments: &mut Arguments,
    ) -> Result<Self, TemplateError>;
}

impl<'ctx, Ctx> FunctionArg<'ctx, Ctx> for &'ctx TemplateEngine<Ctx> {
    fn from_arguments(
        engine: &'ctx TemplateEngine<Ctx>,
        _: &'ctx Ctx,
        _: &mut Arguments,
    ) -> Result<Self, TemplateError> {
        Ok(engine)
    }
}

impl<'ctx, Ctx> FunctionArg<'ctx, Ctx> for &'ctx Ctx {
    fn from_arguments(
        _: &'ctx TemplateEngine<Ctx>,
        context: &'ctx Ctx,
        _: &mut Arguments,
    ) -> Result<Self, TemplateError> {
        Ok(context)
    }
}

/// TODO
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

/// Wrapper for a keyword argument struct, which will be deserialized from a
/// a mapping of keywords. Kwargs should only be used for additional options to
/// a function that are not required. As such, the struct should include
/// `#[serde(default)]` so it can be deserialized when keyword args are missing.
struct Kwargs<T>(T);

impl<'a, 'de, Ctx, T> FunctionArg<'a, Ctx> for Kwargs<T>
where
    T: 'a + Deserialize<'de>,
{
    fn from_arguments(
        _: &'a TemplateEngine<Ctx>,
        _: &'a Ctx,
        arguments: &mut Arguments,
    ) -> Result<Self, TemplateError> {
        let kwargs = mem::take(&mut arguments.keyword);
        // Use a generic deserializer to convert the kwargs as a mapping
        let deserializer = MapDeserializer::new(kwargs.into_iter());
        T::deserialize(deserializer).map(Kwargs)
    }
}
