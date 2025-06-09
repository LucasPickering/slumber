//! Template expression definitions and evaluation

#[cfg(test)]
use crate::test_util;
use crate::{Arguments, TemplateContext, TemplateError, Value};
use derive_more::{Deref, Display, From};
use futures::{FutureExt, future};
use indexmap::IndexMap;

type RenderResult = Result<Value, TemplateError>;

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
    /// Call to a plain function (**not** a filter)
    Call(FunctionCall),
    /// Data piped to another function: `name | trim()`. The expression on the
    /// left will be passed as the first argument to the function call on the
    /// right
    Pipe {
        expression: Box<Self>,
        call: FunctionCall,
    },
}

impl Expression {
    /// Render this expression to bytes
    #[expect(clippy::manual_async_fn, reason = "Doesn't work with recursion")]
    pub(crate) fn render<Ctx: TemplateContext>(
        &self,
        context: &Ctx,
    ) -> impl Future<Output = RenderResult> + Send {
        async move {
            match self {
                Self::Literal(literal) => Ok(literal.into()),
                Self::Array(expressions) => {
                    // Render each inner expression
                    let values = future::try_join_all(
                        expressions
                            .iter()
                            .map(|expression| expression.render(context)),
                    )
                    // Box for recursion
                    .boxed()
                    .await?;
                    Ok(Value::Array(values))
                }
                Self::Field(identifier) => context.get(identifier).await,
                Self::Call(call) => {
                    let arguments = call.render_arguments(context).await?;
                    context.call(&call.function, arguments).await
                }
                Self::Pipe { expression, call } => {
                    // Box for recursion
                    let value = expression.render(context).boxed().await?;
                    let mut arguments = call.render_arguments(context).await?;
                    // Pipe the filter value in as the first positional argument
                    arguments.position.push_front(value);
                    context.call(&call.function, arguments).await
                }
            }
        }
    }
}

impl From<String> for Expression {
    fn from(value: String) -> Self {
        Self::Literal(Literal::from(value))
    }
}

/// Literal primitive value
#[derive(Clone, Debug, From, PartialEq)]
#[cfg_attr(test, derive(proptest_derive::Arbitrary))]
pub enum Literal {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
}

impl From<&str> for Literal {
    fn from(value: &str) -> Self {
        Self::String(value.to_owned())
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
    async fn render_arguments<'ctx, Ctx: TemplateContext>(
        &self,
        context: &'ctx Ctx,
    ) -> Result<Arguments<'ctx, Ctx>, TemplateError> {
        // Render all position and keyword arguments concurrently
        let position_future = future::try_join_all(
            self.position
                .iter()
                .map(|expression| expression.render(context)),
        );
        let keyword_future = future::try_join_all(self.keyword.iter().map(
            |(name, expression)| async {
                let value = expression.render(context).await?;
                Ok((name.to_string(), value))
            },
        ));
        let (position, keyword) =
            future::try_join(position_future, keyword_future)
                // Box for recursion
                .boxed()
                .await?;
        Ok(Arguments {
            context,
            position: position.into(),
            keyword: keyword.into_iter().collect(),
        })
    }
}

/// An identifier that can be used in a template key. A valid identifier is
/// any non-empty string that contains only alphanumeric characters, `-`, or
/// `_`.
///
/// Construct via [FromStr](std::str::FromStr)
#[derive(Clone, Debug, Deref, Default, Display, Eq, Hash, PartialEq)]
#[cfg_attr(test, derive(proptest_derive::Arbitrary))]
pub struct Identifier(
    #[cfg_attr(test, proptest(regex = "[a-zA-Z0-9-_]+"))] pub(crate) String,
);

/// A shortcut for creating identifiers from static strings. Since the string
/// is defined in code we're assuming it's valid.
impl From<&'static str> for Identifier {
    fn from(value: &'static str) -> Self {
        Self(value.parse().unwrap())
    }
}
