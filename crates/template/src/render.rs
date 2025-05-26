//! Template rendering implementation

use crate::{
    Expression, FunctionCall, Literal, TemplateContext, TemplateError, Value,
    function::Arguments,
};
use futures::{FutureExt, future};

type RenderResult = Result<Value, TemplateError>;

impl Expression {
    /// Render this expression to bytes
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
            keyword,
        })
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
        }
    }
}
