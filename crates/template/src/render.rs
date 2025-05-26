//! Template rendering implementation

use crate::{
    Expression, FunctionCall, Literal, TemplateContext, TemplateEngine,
    TemplateError, Value, function::Arguments,
};
use futures::{FutureExt, future};

// TODO partial function application instead of filters?

type RenderResult = Result<Value, TemplateError>;

impl Expression {
    /// Render this expression to bytes
    pub(crate) fn render<Ctx: TemplateContext>(
        &self,
        engine: &TemplateEngine<Ctx>,
        context: &Ctx,
    ) -> impl Future<Output = RenderResult> + Send {
        async move {
            match self {
                Self::Literal(literal) => Ok(literal.into()),
                Self::Array(expressions) => {
                    // Render each inner expression
                    let values =
                        future::try_join_all(expressions.iter().map(
                            |expression| expression.render(engine, context),
                        ))
                        // Box for recursion
                        .boxed()
                        .await?;
                    Ok(Value::Array(values))
                }
                Self::Field(identifier) => {
                    context.get(identifier, engine).await
                }
                Self::Call(call) => {
                    let function =
                        engine.get_function(call.function.as_str())?;
                    let arguments =
                        call.render_arguments(engine, context).await?;
                    function.invoke(arguments)
                }
                Self::Pipe { expression, call } => {
                    // Box for recursion
                    let value =
                        expression.render(engine, context).boxed().await?;
                    let function =
                        engine.get_function(call.function.as_str())?;
                    let mut arguments =
                        call.render_arguments(engine, context).await?;
                    // Pipe the filter value in as the first positional argument
                    arguments.position.push_front(value);
                    function.invoke(arguments)
                }
            }
        }
    }
}

impl FunctionCall {
    async fn render_arguments<'a, Ctx: TemplateContext>(
        &self,
        engine: &'a TemplateEngine<Ctx>,
        context: &'a Ctx,
    ) -> Result<Arguments<'a, Ctx>, TemplateError> {
        // Render all position and keyword arguments concurrently
        let position_future = future::try_join_all(
            self.position
                .iter()
                .map(|expression| expression.render(engine, context)),
        );
        let keyword_future = future::try_join_all(self.keyword.iter().map(
            |(name, expression)| async {
                let value = expression.render(engine, context).await?;
                Ok((name.to_string(), value))
            },
        ));
        let (position, keyword) =
            future::try_join(position_future, keyword_future)
                // Box for recursion
                .boxed()
                .await?;
        Ok(Arguments {
            engine,
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
