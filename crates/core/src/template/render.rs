//! Template rendering implementation

use crate::{
    collection::{RecipeId, RequestTrigger},
    http::{Exchange, RequestSeed, ResponseRecord},
    template::{
        error::RenderResultExt, functions::call_fn, RenderContext, RenderError,
        Template, TriggeredRequestError,
    },
    util::FutureCache,
};
use bytes::Bytes;
use chrono::Utc;
use futures::future;
use hcl::{
    expr::{
        Conditional, FuncCall, Operation, TemplateExpr, Traversal,
        TraversalOperator,
    },
    template::{Directive, Element, Interpolation},
    Expression, ObjectKey,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

// TODO support overrides

// TODO share constants with deserialzation
const LOCALS: &str = "locals";
const PROFILE: &str = "profile";

impl Template {
    /// Render the template using values from the given context. If any chunk
    /// failed to render, return an error. The template is rendered as bytes.
    /// Use [Self::render_string] if you want the bytes converted to a string.
    /// TODO update command
    pub async fn render_bytes(
        &self,
        context: &RenderContext,
    ) -> Result<Vec<u8>, RenderError> {
        let value = self.0.render(context).await?;
        // TODO should we use infallible conversion?
        try_value_to_bytes(value)
    }

    /// Render the template using values from the given context. If any chunk
    /// failed to render, return an error. The rendered template will be
    /// converted from raw bytes to UTF-8. If it is not valid UTF-8, return an
    /// error.
    /// TODO update command
    pub async fn render_string(
        &self,
        context: &RenderContext,
    ) -> Result<String, RenderError> {
        let value = self.0.render(context).await?;
        // TODO should we use infallible conversion?
        try_value_to_string(value)
    }

    /// TODO
    pub async fn render_json(
        &self,
        context: &RenderContext,
    ) -> Result<serde_json::Value, RenderError> {
        let value = self.0.render(context).await?;
        hcl::from_value::<serde_json::Value, _>(value).map_err(|error| todo!())
    }
}

/// State for a render group, which consists of one or more related renders
/// (e.g. all the template renders for a single recipe). This state is stored in
/// the template context.
///
/// Expression evaluation is performed lazily. This struct is the source of
/// truth for which lazy evaluations have begun/completed, so we can dedupe
/// those evaluations across a render group.
#[derive(Debug, Default)]
pub struct RenderGroupState {
    /// Any field from the `locals` block that we've started to evaluate
    locals: FutureCache<String, RenderResult>,
    /// Any field from the profile that we've started to evaluate
    profile: FutureCache<String, RenderResult>,
}

/// Outcome of rendering an expression
pub type RenderResult = Result<RenderValue, RenderError>;

/// An abstraction for an HCL expression that can be rendered to bytes. This is
/// very similar to the [Evaluate](hcl::eval::Evaluate) trait, with a few key
/// differences:
/// - We return bytes instead of an HCL value (HCL values _cannot_ be arbitrary
///   bytes, which means it can't represent non-UTF-8 request bodies)
/// - Rendering is async
/// - Traversal such as `profile.x` and `locals.x` is lazy instead of eager, so
///   that we don't have to evaluate ALL profile/local fields for every render
trait Render {
    async fn render(&self, context: &RenderContext) -> RenderResult;
}

impl Render for Expression {
    async fn render(&self, context: &RenderContext) -> RenderResult {
        match self {
            Expression::Null => Ok(RenderValue::Null),
            Expression::Bool(b) => Ok(RenderValue::Bool(*b)),
            Expression::Number(number) => Ok(RenderValue::Number(*number)),
            Expression::String(s) => Ok(RenderValue::String(s.clone())),
            Expression::Array(vec) => {
                // Concurrency!!
                // TODO provide error context
                let elements = future::try_join_all(
                    vec.iter().map(|expr| expr.render(context)),
                )
                .await?;
                Ok(RenderValue::Array(elements))
            }
            Expression::Object(map) => {
                let elements = future::try_join_all(map.iter().map(
                    |(key, value)| async move {
                        let (key, value) = future::try_join(
                            // TODO provide context specifically to key/value?
                            key.render(context),
                            value.render(context),
                        )
                        .await
                        .context(format!("field `{key}`"))?;
                        // TODO is this the correct behavior? check spec/hcl-rs
                        let key = try_value_to_string(key)?;
                        Ok((key, value))
                    },
                ))
                .await?;
                Ok(RenderValue::Object(elements.into_iter().collect()))
            }
            Expression::TemplateExpr(template_expr) => {
                template_expr.render(context).await
            }
            Expression::Variable(variable) => {
                // Global variables are not supported
                Err(RenderError::GlobalVariable {
                    variable: variable.as_str().to_owned(),
                })
            }
            Expression::Traversal(traversal) => traversal.render(context).await,
            Expression::FuncCall(func_call) => func_call.render(context).await,
            Expression::Parenthesis(expression) => {
                Box::pin(expression.render(context)).await
            }
            Expression::Conditional(conditional) => {
                conditional.render(context).await
            }
            Expression::Operation(operation) => operation.render(context).await,
            Expression::ForExpr(for_expr) => todo!(),
            _ => todo!(),
        }
    }
}

impl Render for ObjectKey {
    async fn render(&self, context: &RenderContext) -> RenderResult {
        match self {
            ObjectKey::Identifier(identifier) => {
                Ok(identifier.as_str().to_owned().into())
            }
            ObjectKey::Expression(expression) => {
                expression.render(context).await
            }
            _ => todo!(),
        }
    }
}

impl Render for TemplateExpr {
    async fn render(&self, context: &RenderContext) -> RenderResult {
        // Parse the string as a template
        let template =
            hcl::Template::from_expr(self).map_err(|error| todo!())?;
        // Render each element independently
        let chunks = future::try_join_all(
            template
                .elements()
                .iter()
                .map(|element| element.render(context)),
        )
        .await?;
        let joined =
            chunks.into_iter().map(value_to_string).collect::<String>();
        Ok(joined.into())
    }
}

impl Render for Element {
    async fn render(&self, context: &RenderContext) -> RenderResult {
        match self {
            Element::Literal(s) => Ok(s.clone().into()),
            Element::Interpolation(interpolation) => {
                interpolation.render(context).await
            }
            Element::Directive(directive) => directive.render(context).await,
        }
    }
}

impl Render for Interpolation {
    async fn render(&self, context: &RenderContext) -> RenderResult {
        // TODO apply strip
        self.expr.render(context).await
    }
}

impl Render for Directive {
    async fn render(&self, context: &RenderContext) -> RenderResult {
        todo!()
    }
}

impl Render for Traversal {
    async fn render(&self, context: &RenderContext) -> RenderResult {
        // TODO support multiple levels of traversal?
        match &self.expr {
            Expression::Variable(variable) if variable.as_str() == LOCALS => {
                // TODO de-dupe with profile case
                let [TraversalOperator::GetAttr(field)] =
                    self.operators.as_slice()
                else {
                    todo!("return error")
                };
                let field = field.as_str();
                let locals = &context.collection.locals;

                // We're the first to ask for this field. Render it ourselves,
                // and share the value back when we're done
                let expression = locals.get(field).ok_or_else(|| {
                    RenderError::VariableUnknown {
                        variable: field.to_owned(),
                    }
                })?;

                // Evaluate this field via the cache. If it's already been
                // evaluated, our future will just be dropped
                context
                    .state
                    .locals
                    .get_or_init(
                        field.to_owned(),
                        Box::pin(expression.render(context)),
                    )
                    .await
                    .context(format!("`{LOCALS}.{field}`"))
            }
            Expression::Variable(variable) if variable.as_str() == PROFILE => {
                let [TraversalOperator::GetAttr(field)] =
                    self.operators.as_slice()
                else {
                    todo!("return error")
                };
                let field = field.as_str();
                let profile = context.profile()?;

                // We're the first to ask for this field. Render it ourselves,
                // and share the value back when we're done
                let expression = profile.data.get(field).ok_or_else(|| {
                    RenderError::VariableUnknown {
                        variable: field.to_owned(),
                    }
                })?;

                // Evaluate this field via the cache. If it's already been
                // evaluated, our future will just be dropped
                context
                    .state
                    .profile
                    .get_or_init(
                        field.to_owned(),
                        Box::pin(expression.0.render(context)),
                    )
                    .await
                    .context(format!("`{PROFILE}.{field}`"))
            }
            // For any other type of expression, we don't know how to traverse
            // it lazily, so we'll render the whole thing and traverse the value
            expr => {
                let value =
                    Box::pin(expr.render(context)).await.context("TODO")?;
                traverse(value, &self.operators, context).await
            }
        }
    }
}

impl Render for FuncCall {
    async fn render(&self, context: &RenderContext) -> RenderResult {
        if self.expand_final {
            todo!("varargs not allowed")
        }
        if self.name.is_namespaced() {
            return Err(RenderError::FunctionUnknown {
                name: self.name.to_string(),
            });
        }
        let name = self.name.name.as_str();
        // All functions expect a single object as the argument
        let [arg] = self.args.as_slice() else { todo!() };
        // This _should_ render to an object, but if not we'll hit an error
        // during deserialization
        let arg = Box::pin(arg.render(context))
            .await
            .context(format!("arguments to function `{name}`"))?;
        call_fn(name, arg, context).await
    }
}

impl Render for Conditional {
    async fn render(&self, context: &RenderContext) -> RenderResult {
        todo!()
    }
}

impl Render for Operation {
    async fn render(&self, context: &RenderContext) -> RenderResult {
        todo!()
    }
}

/// TODO
async fn traverse(
    value: RenderValue,
    operators: &[TraversalOperator],
    context: &RenderContext,
) -> RenderResult {
    // TODO handle multiple levels of traversal
    match value {
        RenderValue::Null => todo!(),
        RenderValue::Bool(_) => todo!(),
        RenderValue::Number(number) => todo!(),
        RenderValue::String(_) => todo!(),
        RenderValue::Capsule(bytes) => todo!(),
        RenderValue::Array(mut vec) => {
            let [TraversalOperator::Index(index)] = operators else {
                todo!("return error")
            };
            let RenderValue::Number(index) =
                Box::pin(index.render(context)).await.context("TODO")?
            else {
                todo!("error")
            };
            let Some(index) = index.as_u64() else {
                todo!("error")
            };
            // TODO bounds check
            Ok(vec.swap_remove(index as usize))
        }
        RenderValue::Object(index_map) => todo!(),
    }
}

/// An HCL value, extended to support binary values
pub type RenderValue = hcl::Value<Bytes>;

impl RenderContext {
    /// Get the most recent response for a profile+recipe pair
    pub(super) async fn get_latest_response(
        &self,
        recipe_id: &RecipeId,
        trigger: RequestTrigger,
    ) -> Result<ResponseRecord, RenderError> {
        // Defer loading the most recent exchange until we know we'll need it
        let get_latest = || -> Result<Option<Exchange>, RenderError> {
            self.database
                .get_latest_request(
                    self.selected_profile.as_ref().into(),
                    recipe_id,
                )
                .map_err(|error| RenderError::Database(error.into()))
        };

        // Helper to execute the request, if triggered
        let send_request = || async {
            // There are 3 different ways we can generate the request config:
            // 1. Default (enable all query params/headers)
            // 2. Load from UI state for both TUI and CLI
            // 3. Load from UI state for TUI, enable all for CLI
            // These all have their own issues:
            // 1. Triggered request doesn't necessarily match behavior if user
            //  were to execute the request themself
            // 2. CLI behavior is silently controlled by UI state
            // 3. TUI and CLI behavior may not match
            // All 3 options are unintuitive in some way, but 1 is the easiest
            // to implement so I'm going with that for now.
            let build_options = Default::default();

            // Shitty try block
            async {
                let http_engine = self
                    .http_engine
                    .as_ref()
                    .ok_or(TriggeredRequestError::NotAllowed)?;
                // Pin needed for rendering recursion
                // TODO how to nest this correct?
                let ticket = Box::pin(http_engine.build(
                    RequestSeed::new(recipe_id.clone(), build_options),
                    self,
                ))
                .await
                .map_err(|error| TriggeredRequestError::Build(error.into()))?;
                ticket
                    .send(&self.database)
                    .await
                    .map_err(|error| TriggeredRequestError::Send(error.into()))
            }
            .await
            .map_err(|error| RenderError::Trigger {
                recipe_id: recipe_id.clone(),
                error,
            })
        };

        let exchange = match trigger {
            RequestTrigger::Never => {
                get_latest()?.ok_or(RenderError::ResponseMissing {
                    recipe_id: recipe_id.clone(),
                })?
            }
            RequestTrigger::NoHistory => {
                // If a exchange is present in history, use that. If not, fetch
                if let Some(exchange) = get_latest()? {
                    exchange
                } else {
                    send_request().await?
                }
            }
            RequestTrigger::Expire(duration) => match get_latest()? {
                Some(exchange)
                    if exchange.end_time + duration >= Utc::now() =>
                {
                    exchange
                }
                _ => send_request().await?,
            },
            RequestTrigger::Always => send_request().await?,
        };

        // Safe because we just created the exchange and it hasn't been shared
        Ok(
            Arc::into_inner(exchange.response)
                .expect("Arc has not been shared"),
        )
    }
}

/// Trim whitespace from rendered output
///
/// TODO explain difference between this and HCL's Strip
#[derive(Copy, Clone, Debug, Default, Serialize, Deserialize)]
#[cfg_attr(any(test, feature = "test"), derive(PartialEq))]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum TrimMode {
    /// Do not trim the output
    None,
    /// Trim the start of the output
    Start,
    /// Trim the end of the output
    End,
    /// Trim the start and end of the output
    #[default]
    // TODO this is a change from 2.0, document it/handle in import
    Both,
}

impl TrimMode {
    /// Apply whitespace trimming to string values. If the value is not a valid
    /// string, no trimming is applied
    pub fn apply(self, value: Vec<u8>) -> Vec<u8> {
        // Theoretically we could strip whitespace-looking characters from
        // binary data, but if the whole thing isn't a valid string it doesn't
        // really make any sense to.
        let Ok(s) = std::str::from_utf8(&value) else {
            return value;
        };
        match self {
            Self::None => value,
            Self::Start => s.trim_start().into(),
            Self::End => s.trim_end().into(),
            Self::Both => s.trim().into(),
        }
    }
}

/// TODO
pub fn bytes_to_value(bytes: Vec<u8>) -> RenderValue {
    // TODO remove double conversion bytes -> vec -> bytes
    String::from_utf8(bytes)
        .map(RenderValue::String)
        .unwrap_or_else(|error| RenderValue::Capsule(error.into_bytes().into()))
}

/// TODO
pub fn value_to_string(value: RenderValue) -> String {
    if let RenderValue::String(s) = value {
        // Don't include quotes
        s
    } else {
        value.to_string()
    }
}

/// TODO
fn try_value_to_string(value: RenderValue) -> Result<String, RenderError> {
    match value {
        RenderValue::Null => Ok("null".into()),
        RenderValue::Bool(b) => Ok(b.to_string()),
        RenderValue::Number(number) => Ok(number.to_string()),
        RenderValue::String(s) => Ok(s),
        RenderValue::Capsule(bytes) => {
            // TODO drop this
            String::from_utf8(bytes.to_vec()).map_err(RenderError::InvalidUtf8)
        }
        RenderValue::Array(_) => Err(RenderError::ExpectedScalar { value }),
        RenderValue::Object(_) => Err(RenderError::ExpectedScalar { value }),
    }
}

/// TODO
fn try_value_to_bytes(value: RenderValue) -> Result<Vec<u8>, RenderError> {
    if let RenderValue::Capsule(bytes) = value {
        Ok(bytes.to_vec())
    } else {
        // Use the value->string conversion, then take the string bytes.
        // This will fail for array and object types
        try_value_to_string(value).map(String::into_bytes)
    }
}
