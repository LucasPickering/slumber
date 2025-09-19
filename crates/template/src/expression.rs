//! Template expression definitions and evaluation

#[cfg(test)]
use crate::test_util;
use crate::{
    Arguments, Context, RenderError, Stream, Value, error::RenderErrorContext,
    util::FieldCacheOutcome,
};
use bytes::Bytes;
use derive_more::{Deref, Display, From};
use futures::{
    FutureExt,
    future::{self, try_join},
};
use indexmap::IndexMap;

type RenderResult = Result<Stream, RenderError>;

/// A dynamic segment of a template that will be computed at render time.
/// Expressions are derived from the template context and may include external
/// data such as loading a file.
#[derive(Clone, Debug, PartialEq)]
pub enum Expression {
    /// A literal value such as `3`, `false`, or `"hello"`
    Literal(Literal),
    /// Field access, as defined by [TemplateContext::get]
    Field(Identifier),
    /// Array literal: `[1, "hello", f()]`
    Array(Vec<Self>),
    /// Object literal: `{"a": 1}`. Store a vec here instead of a map because
    /// we don't want to deduplicate keys until after evaluating them
    Object(Vec<(Self, Self)>),
    /// Call to a plain function (**not** a filter)
    Call(FunctionCall),
    /// Data piped to another function: `name | trim()`. The expression on the
    /// left will be passed as the last positional argument to the function call
    /// on the right
    Pipe {
        expression: Box<Self>,
        call: FunctionCall,
    },
}

impl Expression {
    /// Render this expression to bytes
    #[expect(clippy::manual_async_fn, reason = "Doesn't work with recursion")]
    pub(crate) fn render<Ctx: Context>(
        &self,
        context: &Ctx,
    ) -> impl Future<Output = RenderResult> + Send {
        async move {
            match self {
                Self::Literal(literal) => Ok(literal.into()),
                Self::Array(expressions) => {
                    // Render each inner expression
                    let values =
                        future::try_join_all(expressions.iter().map(
                            |expression| expression.render_value(context),
                        ))
                        // Box for recursion
                        .boxed()
                        .await?;
                    Ok(Value::Array(values).into())
                }
                Self::Object(entries) => {
                    let pairs: Vec<(String, Value)> = future::try_join_all(
                        entries.iter().map(|(key, value)| {
                            let key_future = async move {
                                let key = key.render_value(context).await?;
                                // Keys must be strings, so convert here
                                key.try_into_string().map_err(|error| {
                                    RenderError::Value(error.error)
                                })
                            };
                            try_join(key_future, value.render_value(context))
                        }),
                    )
                    .boxed() // Box for recursion
                    .await?;
                    // Keys will be deduped here, with the last taking priority
                    Ok(Value::Object(IndexMap::from_iter(pairs)).into())
                }
                Self::Field(field) => Self::render_field(field, context).await,
                Self::Call(call) => call.call(context, None).await,
                Self::Pipe { expression, call } => {
                    // Compute the left hand side first. Box for recursion
                    let value =
                        expression.render_value(context).boxed().await?;
                    call.call(context, Some(value)).await
                }
            }
        }
    }

    /// Render the value of a field. This will apply caching, so that a field
    /// never has to be rendered more than once for a given context.
    async fn render_field<Ctx: Context>(
        field: &Identifier,
        context: &Ctx,
    ) -> RenderResult {
        // Check the field cache to see if this value is already being computed
        // somewhere else. If it is, we'll block on that and re-use the result.
        // If not, we get a guard back, meaning we're responsible for the
        // computation. At the end, we'll write back to the guard so everyone
        // else can copy our homework.
        let cache = context.field_cache();
        let guard = match cache.get_or_init(field.clone()).await {
            FieldCacheOutcome::Hit(stream) => return Ok(stream),
            FieldCacheOutcome::Miss(guard) => guard,
            // The future responsible for writing to the guard failed. Cloning
            // errors is annoying so we return an empty response here. The
            // initial error should've been returned elsewhere so that can be
            // used instead.
            FieldCacheOutcome::NoResponse => {
                return Err(RenderError::CacheFailed {
                    field: field.clone(),
                });
            }
        };

        // This value hasn't been rendered yet - ask the context to evaluate it
        let mut stream = context.get_field(field).await?;
        // If streaming isn't supported here, convert to a value before caching,
        // so that the stream isn't evaluated multiple times unless necessary
        if !context.can_stream() {
            stream = stream.resolve().await?.into();
        }

        // Store value in the cache so other references to this field can use it
        guard.set(stream.clone());
        Ok(stream)
    }

    /// Render this expression, resolving any stream to a concrete value.
    async fn render_value<Ctx: Context>(
        &self,
        context: &Ctx,
    ) -> Result<Value, RenderError> {
        self.render(context).await?.resolve().await
    }

    /// Build a function call expression. Any keyword arguments with `None`
    /// values will be omitted
    pub fn call(
        function_name: &'static str,
        position: impl IntoIterator<Item = Expression>,
        keyword: impl IntoIterator<Item = (&'static str, Option<Expression>)>,
    ) -> Self {
        Self::Call(FunctionCall::new(function_name, position, keyword))
    }

    /// Build a pipe expression with this expression as the left-hand side and
    /// a function call on the right-hand side
    #[must_use]
    pub fn pipe(
        self,
        rhs_name: &'static str,
        rhs_position: impl IntoIterator<Item = Expression>,
        rhs_keyword: impl IntoIterator<Item = (&'static str, Option<Expression>)>,
    ) -> Self {
        Self::Pipe {
            expression: Box::new(self),
            call: FunctionCall::new(rhs_name, rhs_position, rhs_keyword),
        }
    }
}

impl From<bool> for Expression {
    fn from(b: bool) -> Self {
        Self::Literal(Literal::Boolean(b))
    }
}

impl From<f64> for Expression {
    fn from(f: f64) -> Self {
        Self::Literal(Literal::Float(f))
    }
}

impl From<i64> for Expression {
    fn from(i: i64) -> Self {
        Self::Literal(Literal::Integer(i))
    }
}

impl From<Literal> for Expression {
    fn from(literal: Literal) -> Self {
        Self::Literal(literal)
    }
}

impl From<String> for Expression {
    fn from(value: String) -> Self {
        Self::Literal(Literal::from(value))
    }
}

impl From<&str> for Expression {
    fn from(value: &str) -> Self {
        Self::Literal(Literal::from(value))
    }
}

impl From<&'static [u8]> for Expression {
    fn from(value: &'static [u8]) -> Self {
        Self::Literal(Literal::Bytes(Bytes::from(value.to_vec())))
    }
}

impl<const N: usize> From<&'static [u8; N]> for Expression {
    fn from(value: &'static [u8; N]) -> Self {
        value.as_slice().into()
    }
}

impl From<Vec<Expression>> for Expression {
    fn from(values: Vec<Expression>) -> Self {
        Self::Array(values)
    }
}

impl FromIterator<Expression> for Expression {
    fn from_iter<T: IntoIterator<Item = Self>>(iter: T) -> Self {
        Self::Array(Vec::from_iter(iter))
    }
}

/// Literal primitive value
#[derive(Clone, Debug, From, PartialEq)]
#[cfg_attr(test, derive(proptest_derive::Arbitrary))]
pub enum Literal {
    Null,
    Boolean(bool),
    Integer(i64),
    Float(f64),
    String(String),
    Bytes(#[cfg_attr(test, proptest(strategy = "test_util::bytes()"))] Bytes),
}

impl From<&str> for Literal {
    fn from(value: &str) -> Self {
        Self::String(value.to_owned())
    }
}

impl<const N: usize> From<&[u8; N]> for Literal {
    fn from(value: &[u8; N]) -> Self {
        Self::Bytes(Bytes::from(value.to_vec()))
    }
}

/// Function call in a template expression: `f(true, 0, kwarg0="hello")`
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(test, derive(proptest_derive::Arbitrary))]
pub struct FunctionCall {
    pub(crate) function: Identifier,
    /// Positional arguments
    #[cfg_attr(
        test,
        proptest(
            strategy = "proptest::collection::vec(test_util::expression_arbitrary(), 0..=3)"
        )
    )]
    pub(crate) position: Vec<Expression>,
    /// Keyword arguments
    ///
    /// Must be an indexmap so evaluation order will match lexical order
    #[cfg_attr(
        test,
        proptest(
            strategy = "test_util::index_map(Identifier::arbitrary(), test_util::expression_arbitrary(), 0..=3)"
        )
    )]
    pub(crate) keyword: IndexMap<Identifier, Expression>,
}

impl FunctionCall {
    /// Build a new function call from name+arguments
    fn new(
        function_name: &'static str,
        position: impl IntoIterator<Item = Expression>,
        keyword: impl IntoIterator<Item = (&'static str, Option<Expression>)>,
    ) -> Self {
        FunctionCall {
            function: function_name.into(),
            position: position.into_iter().collect(),
            keyword: keyword
                .into_iter()
                // kwargs are inherently optional, so drop ones with no value
                .filter_map(|(name, value)| Some((name.into(), value?)))
                .collect(),
        }
    }

    /// Render arguments and call the function
    async fn call<Ctx: Context>(
        &self,
        context: &Ctx,
        piped_argument: Option<Value>,
    ) -> RenderResult {
        // Provide context to the error
        let map_error = |error: RenderError| {
            error.context(RenderErrorContext::Function(self.function.clone()))
        };

        let mut arguments =
            self.render_arguments(context).await.map_err(map_error)?;
        if let Some(piped_argument) = piped_argument {
            // Pipe the filter value in as the last positional argument
            arguments.push_piped(piped_argument);
        }
        context
            .call(&self.function, arguments)
            .await
            .map_err(map_error)
    }

    /// Render each argument passed in this function call
    async fn render_arguments<'ctx, Ctx: Context>(
        &self,
        context: &'ctx Ctx,
    ) -> Result<Arguments<'ctx, Ctx>, RenderError> {
        // Render all position and keyword arguments concurrently. We attach
        // error context to any failures so the user know which arg failed to
        // render
        let position_future =
            future::try_join_all(self.position.iter().enumerate().map(
                |(index, expression)| async move {
                    expression.render_value(context).await.map_err(|error| {
                        error.context(RenderErrorContext::ArgumentRender {
                            argument: index.to_string(),
                            expression: expression.clone(),
                        })
                    })
                },
            ));
        let keyword_future = future::try_join_all(self.keyword.iter().map(
            |(name, expression)| async {
                let value = expression.render_value(context).await.map_err(
                    |error| {
                        error.context(RenderErrorContext::ArgumentRender {
                            argument: name.to_string(),
                            expression: expression.clone(),
                        })
                    },
                )?;
                Ok((name.to_string(), value))
            },
        ));
        let (position, keyword) =
            future::try_join(position_future, keyword_future)
                // Box for recursion
                .boxed()
                .await?;
        Ok(Arguments::new(
            context,
            position.into(),
            keyword.into_iter().collect(),
        ))
    }
}

/// An identifier that can be used in a template key. A valid identifier is
/// any non-empty string that contains only alphanumeric characters, `-`, or
/// `_`. The first character must be a letter or underscore. Hyphens and numbers
/// are not allowed first to avoid ambiguity with number literals.
///
/// Construct via [FromStr](std::str::FromStr)
#[derive(Clone, Debug, Deref, Default, Display, Eq, Hash, PartialEq)]
#[cfg_attr(test, derive(proptest_derive::Arbitrary))]
pub struct Identifier(
    // \p{L} will spit out valid unicode letters
    // https://www.unicode.org/reports/tr44/tr44-24.html#General_Category_Values
    #[cfg_attr(test, proptest(regex = r"[\p{L}_][\p{L}0-9-_]*"))]
    pub(crate)  String,
);

/// A shortcut for creating identifiers from static strings. Since the string
/// is defined in code we're assuming it's valid.
impl From<&'static str> for Identifier {
    fn from(value: &'static str) -> Self {
        Self(value.parse().unwrap())
    }
}
