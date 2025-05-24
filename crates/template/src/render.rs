//! Template rendering implementation

use crate::{
    Expression, FunctionCall, Literal, TemplateContext, TemplateEngine,
    TemplateError, Value, function::Arguments,
};
use futures::future;

// TODO partial function application instead of filters?

pub type RenderResult = Result<Value, TemplateError>;

impl Expression {
    /// Render this expression to bytes
    pub(crate) async fn render<Ctx: TemplateContext>(
        &self,
        engine: &TemplateEngine<Ctx>,
        context: &Ctx,
    ) -> RenderResult {
        match self {
            Self::Literal(literal) => Ok(literal.into()),
            Self::Array(expressions) => {
                // Render each inner expression
                let values = future::try_join_all(
                    expressions
                        .iter()
                        .map(|expression| expression.render(engine, context)),
                )
                .await?;
                Ok(Value::Array(values))
            }
            Self::Field(identifier) => context.get(identifier, engine).await,
            Self::Call(call) => {
                let producer = engine
                    .get_function(call.function.as_str())?
                    .as_producer()?;
                let arguments = call.render_arguments(engine, context).await?;
                producer.call(engine, context, arguments).await
            }
            Self::Filter {
                expression,
                filter: call,
            } => {
                let value = expression.render(engine, context).await?;
                let filter =
                    engine.get_function(call.function.as_str())?.as_filter()?;
                let arguments = call.render_arguments(engine, context).await?;
                filter.call(engine, context, value, arguments).await
            }
        }
    }
}

impl FunctionCall {
    async fn render_arguments<Ctx: TemplateContext>(
        &self,
        engine: &TemplateEngine<Ctx>,
        context: &Ctx,
    ) -> Result<Arguments, TemplateError> {
        // Render all position and keyword arguments concurrently
        let position_future = future::try_join_all(
            self.arguments
                .iter()
                .map(|expression| expression.render(engine, context)),
        );
        let keyword_future = future::try_join_all(self.kwargs.iter().map(
            |(name, expression)| async {
                let value = expression.render(engine, context).await?;
                Ok((name.to_string(), value))
            },
        ));
        let (position, keyword) =
            future::try_join(position_future, keyword_future).await?;
        Ok(Arguments { position, keyword })
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
