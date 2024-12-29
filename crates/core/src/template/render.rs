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
        BinaryOp, Conditional, FuncCall, Operation, TemplateExpr, Traversal,
        TraversalOperator, UnaryOp,
    },
    template::{Directive, Element, Interpolation},
    Expression, Number, ObjectKey, Value,
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
        // TODO should we use infallible conversion?
        self.0.render(context).await?.scalar_to_bytes()
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
        // TODO should we use infallible conversion?
        self.0.render(context).await?.scalar_to_string()
    }

    /// TODO
    pub async fn render_json(
        &self,
        context: &RenderContext,
    ) -> Result<serde_json::Value, RenderError> {
        self.0.render(context).await?.into_json()
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

/// An HCL value, extended to support binary values
///
/// Binary (capsule) values should only ever representing **invalid UTF-8
/// bytes**. The various construction methods ensure that if bytes are UTF-8,
/// we construct a string.
#[derive(Clone, Debug, derive_more::Display, Deserialize)]
#[serde(transparent)]
pub struct RenderValue(Value<Bytes>);

impl RenderValue {
    /// TODO
    async fn traverse(
        self,
        operators: &[TraversalOperator],
        context: &RenderContext,
    ) -> RenderResult {
        // TODO handle multiple levels of traversal
        match self.0 {
            Value::Null => todo!(),
            Value::Bool(_) => todo!(),
            Value::Number(number) => todo!(),
            Value::String(_) => todo!(),
            Value::Capsule(bytes) => todo!(),
            Value::Array(mut vec) => {
                let [TraversalOperator::Index(index)] = operators else {
                    todo!("return error")
                };
                let Value::Number(index) =
                    Box::pin(index.render(context)).await.context("TODO")?.0
                else {
                    todo!("error")
                };
                let Some(index) = index.as_u64() else {
                    todo!("error")
                };
                // TODO bounds check
                Ok(vec.swap_remove(index as usize).into())
            }
            Value::Object(index_map) => todo!(),
        }
    }

    /// Convert this value to a string. If the value is already a string, the
    /// internal value will be returned, i.e. **quotes will be dropped**.
    /// Otherwise, the value will be stringified.
    pub fn into_string(self) -> String {
        if let Value::String(s) = self.0 {
            // Don't include quotes
            s
        } else {
            self.to_string()
        }
    }

    /// TODO
    fn scalar_to_string(self) -> Result<String, RenderError> {
        match self.0 {
            Value::Null => Ok("null".into()),
            Value::Bool(b) => Ok(b.to_string()),
            Value::Number(number) => Ok(number.to_string()),
            Value::String(s) => Ok(s),
            // Capsule is always invalid UTF-8, so we can't convert to a string
            Value::Capsule(_) => Err(RenderError::UnexpectedType {
                expected: "UTF-8 bytes",
                value: self,
            }),
            Value::Array(_) | Value::Object(_) => {
                Err(RenderError::UnexpectedType {
                    expected: "scalar",
                    value: self,
                })
            }
        }
    }

    /// TODO
    fn scalar_to_bytes(self) -> Result<Vec<u8>, RenderError> {
        if let Value::Capsule(bytes) = self.0 {
            Ok(bytes.to_vec())
        } else {
            // Use the value->string conversion, then take the string bytes.
            // This will fail for array and object types
            self.scalar_to_string().map(String::into_bytes)
        }
    }

    /// TODO
    pub fn into_json(self) -> Result<serde_json::Value, RenderError> {
        hcl::from_value::<serde_json::Value, _>(self.0).map_err(|error| todo!())
    }

    fn try_into_bool(self) -> Result<bool, RenderError> {
        match self.0 {
            Value::Bool(b) => Ok(b),
            _ => Err(RenderError::UnexpectedType {
                expected: "bool",
                value: self,
            }),
        }
    }
}

impl From<Value<Bytes>> for RenderValue {
    fn from(value: Value<Bytes>) -> Self {
        Self(value)
    }
}

impl From<RenderValue> for Value<Bytes> {
    fn from(value: RenderValue) -> Self {
        value.0
    }
}

impl From<bool> for RenderValue {
    fn from(b: bool) -> Self {
        Self(Value::Bool(b))
    }
}

impl From<Number> for RenderValue {
    fn from(n: Number) -> Self {
        Self(Value::Number(n))
    }
}

impl From<String> for RenderValue {
    fn from(s: String) -> Self {
        Self(Value::String(s))
    }
}

impl From<Vec<u8>> for RenderValue {
    fn from(bytes: Vec<u8>) -> Self {
        String::from_utf8(bytes)
            .map(RenderValue::from)
            .unwrap_or_else(|error| {
                Self(Value::Capsule(error.into_bytes().into()))
            })
    }
}

impl From<Bytes> for RenderValue {
    fn from(bytes: Bytes) -> Self {
        std::str::from_utf8(&bytes)
            .map(|s| s.to_owned().into())
            .unwrap_or_else(|_| Self(Value::Capsule(bytes)))
    }
}

impl From<&serde_json::Value> for RenderValue {
    fn from(json: &serde_json::Value) -> RenderValue {
        match json {
            serde_json::Value::Null => Value::Null.into(),
            serde_json::Value::Bool(b) => (*b).into(),
            serde_json::Value::Number(number) => todo!(),
            serde_json::Value::String(s) => s.clone().into(),
            serde_json::Value::Array(vec) => {
                vec.iter().map(Self::from).collect()
            }
            serde_json::Value::Object(map) => map
                .iter()
                .map(|(key, value)| (key.clone(), value.into()))
                .collect(),
        }
    }
}

/// Construct an HCL array from a series of values
impl FromIterator<Self> for RenderValue {
    fn from_iter<T: IntoIterator<Item = Self>>(iter: T) -> Self {
        // We need to unwrap each element from RenderValue to hcl::Value, then
        // rewrap the parent
        Self(Value::Array(iter.into_iter().map(Value::from).collect()))
    }
}

/// Construct an HCL object from a series of (key, value)
impl FromIterator<(String, Self)> for RenderValue {
    fn from_iter<T: IntoIterator<Item = (String, Self)>>(iter: T) -> Self {
        // We need to unwrap each element from RenderValue to hcl::Value, then
        // rewrap the parent
        Self(Value::Object(
            iter.into_iter()
                .map(|(key, value)| (key, value.0))
                .collect(),
        ))
    }
}

/// An abstraction for an HCL expression that can be rendered to bytes. This is
/// very similar to the [Evaluate](hcl::eval::Evaluate) trait, with a few key
/// differences:
/// - We return bytes instead of an HCL value (HCL values _cannot_ be arbitrary
///   bytes, which means it can't represent non-UTF-8 request bodies)
/// - Rendering is async
/// - Traversal such as `profile.x` and `locals.x` is lazy instead of eager, so
///   that we don't have to evaluate ALL profile/local fields for every render
pub trait Render {
    #[allow(async_fn_in_trait)]
    async fn render(&self, context: &RenderContext) -> RenderResult;
}

impl Render for Template {
    async fn render(&self, context: &RenderContext) -> RenderResult {
        self.0.render(context).await
    }
}

impl Render for Expression {
    async fn render(&self, context: &RenderContext) -> RenderResult {
        match self {
            Expression::Null => Ok(Value::Null.into()),
            Expression::Bool(b) => Ok((*b).into()),
            Expression::Number(number) => Ok((*number).into()),
            Expression::String(s) => Ok(s.clone().into()),
            Expression::Array(vec) => {
                // Concurrency!!
                // TODO provide error context
                let elements = future::try_join_all(
                    vec.iter().map(|expr| expr.render(context)),
                )
                .await?;
                Ok(elements.into_iter().collect())
            }
            Expression::Object(map) => {
                let elements = future::try_join_all(map.iter().map(
                    |(key, value)| async move {
                        let (key, value) = future::try_join(
                            key.render(context),
                            value.render(context),
                        )
                        .await
                        .context(format!("field `{key}`"))?;
                        // TODO is this the correct behavior? check spec/hcl-rs
                        let key = key.scalar_to_string()?;
                        Ok((key, value.0))
                    },
                ))
                .await?;
                Ok(Value::Object(elements.into_iter().collect()).into())
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
            // These are complicated to implement and probably not that useful
            Expression::ForExpr(_) => Err(RenderError::Unimplemented {
                description: "for expressions".into(),
            }),
            // Required for non_exhaustive enum
            other => unimplemented!("unknown expression type: {other}"),
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
            // Required for non_exhaustive enum
            other => unimplemented!("unknown object key type: {other}"),
        }
    }
}

impl Render for TemplateExpr {
    async fn render(&self, context: &RenderContext) -> RenderResult {
        // Parse the string as a template
        let template = hcl::Template::from_expr(self).map_err(|error| {
            // I don't think this is possible because invalid templates won't
            // pass the original HCL parsing, but I'm not sure
            RenderError::TemplateParse {
                template: self.clone(),
                error: error.into(),
            }
        })?;
        // Render each element independently
        let chunks = future::try_join_all(
            template
                .elements()
                .iter()
                .map(|element| element.render(context)),
        )
        .await?;
        let joined = chunks
            .into_iter()
            .map(RenderValue::into_string)
            .collect::<String>();
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
        match self {
            Directive::If(if_directive) => todo!(),
            Directive::For(for_directive) => todo!(),
        }
    }
}

impl Render for Traversal {
    async fn render(&self, context: &RenderContext) -> RenderResult {
        // TODO support multiple levels of traversal
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
                    RenderError::UndefinedVariable {
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
                    RenderError::UndefinedVariable {
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
                value.traverse(&self.operators, context).await
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
            return Err(RenderError::UndefinedFunction {
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
        // TODO error context?
        let condition = Box::pin(self.cond_expr.render(context))
            .await
            .context("condition")?
            .try_into_bool()?;
        if condition {
            Box::pin(self.true_expr.render(context))
                .await
                .context("true branch")
        } else {
            Box::pin(self.false_expr.render(context))
                .await
                .context("false branch")
        }
    }
}

impl Render for Operation {
    async fn render(&self, context: &RenderContext) -> RenderResult {
        match self {
            Operation::Unary(op) => op.render(context).await,
            Operation::Binary(op) => op.render(context).await,
        }
    }
}

impl Render for UnaryOp {
    async fn render(&self, context: &RenderContext) -> RenderResult {
        use hcl::{UnaryOperator::*, Value::*};
        let value = Box::pin(self.expr.render(context)).await?;

        let value = match (self.operator, value.0) {
            (Not, Bool(v)) => Bool(!v),
            (Neg, Number(n)) => Number(-n),
            (operator, value) => {
                return Err(RenderError::UnexpectedUnary {
                    operator,
                    value: value.into(),
                })
            }
        };
        Ok(value.into())
    }
}

impl Render for BinaryOp {
    async fn render(&self, context: &RenderContext) -> RenderResult {
        use hcl::{BinaryOperator::*, Value::*};

        let lhs_value = Box::pin(self.lhs_expr.render(context)).await?;
        let rhs_value = Box::pin(self.lhs_expr.render(context)).await?;
        let value = match (lhs_value.0, self.operator, rhs_value.0) {
            (lhs, Eq, rhs) => Bool(lhs == rhs),
            (lhs, NotEq, rhs) => Bool(lhs != rhs),
            (Bool(lhs), And, Bool(rhs)) => Bool(lhs && rhs),
            (Bool(lhs), Or, Bool(rhs)) => Bool(lhs || rhs),
            (Number(lhs), LessEq, Number(rhs)) => Bool(lhs <= rhs),
            (Number(lhs), GreaterEq, Number(rhs)) => Bool(lhs >= rhs),
            (Number(lhs), Less, Number(rhs)) => Bool(lhs < rhs),
            (Number(lhs), Greater, Number(rhs)) => Bool(lhs > rhs),
            (Number(lhs), Plus, Number(rhs)) => Number(lhs + rhs),
            (Number(lhs), Minus, Number(rhs)) => Number(lhs - rhs),
            (Number(lhs), Mul, Number(rhs)) => Number(lhs * rhs),
            (Number(lhs), Div, Number(rhs)) => Number(lhs / rhs),
            (Number(lhs), Mod, Number(rhs)) => Number(lhs % rhs),
            (lhs, operator, rhs) => {
                return Err(RenderError::UnexpectedBinary {
                    operator,
                    lhs: lhs.into(),
                    rhs: rhs.into(),
                })
            }
        };
        Ok(value.into())
    }
}

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
