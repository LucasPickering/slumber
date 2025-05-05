//! Render PetitScript recipe values into static strings and bytes for HTTP
//! requests.

use crate::{
    collection::{Collection, Profile, ProfileId, RecipeId},
    http::{Exchange, RequestSeed, TriggeredRequestError},
    util::FutureCache,
};
use anyhow::anyhow;
use async_trait::async_trait;
use bytes::Bytes;
use derive_more::From;
use indexmap::IndexMap;
use petitscript::{Process, Value, value::Function};
use serde::{Deserialize, Serialize};
use slumber_util::ResultTraced;
use std::{
    fmt::{self, Debug, Display},
    str::FromStr,
    sync::Arc,
};
use thiserror::Error;
use tokio::{sync::oneshot, task};
use winnow::{
    ModalResult, Parser,
    ascii::dec_uint,
    combinator::{alt, opt, preceded},
    token::take_while,
};

/// A definition of a how a recipe value should be rendered.
/// [petitscript::Value] to be used in a recipe. Procedures come in two forms:
/// - Static: a predefined value such as a number, string, or object
/// - Dynamic: a function that dynamically generates a value at render time,
///   based on external factors such as files, responses, or user input
///
/// The name "procedure" is fairly arbitrary, but it's specific and unique so
/// it works. This is the successor to Slumber's previous "template" system,
/// which were declarative strings.
#[derive(Clone, Serialize, Deserialize)]
#[serde(into = "Value", from = "Value")]
pub struct Procedure(Value);

impl Procedure {
    pub fn new(value: impl Into<Value>) -> Self {
        Self(value.into())
    }

    /// Programatically build a procedure from an expression. Used for building
    /// expected values in a test assertion
    #[cfg(any(test, feature = "test"))]
    pub fn test(expression: petitscript::ast::Expression) -> Self {
        use petitscript::ast::{FunctionBody, FunctionDefinition};

        Self(Value::Function(Function::user(
            FunctionDefinition::new([], FunctionBody::expression(expression))
                .into(),
            Default::default(),
        )))
    }

    /// [Procedure::test] specifically for template literal expressions. Saves a
    /// level of indentation
    #[cfg(any(test, feature = "test"))]
    pub fn template(
        chunks: impl IntoIterator<Item = petitscript::ast::TemplateChunk>,
    ) -> Self {
        use petitscript::ast::TemplateLiteral;

        Self::test(TemplateLiteral::new(chunks).into())
    }

    /// Is the procedure a function that will be rendered dynamically into
    /// another value?
    pub fn is_dynamic(&self) -> bool {
        matches!(&self.0, Value::Function(_))
    }

    /// Get the inner PS value
    pub fn into_value(self) -> Value {
        self.0
    }
}

impl Debug for Procedure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // For dynamic procedures, ignore the name and captures in the function
        // definition. These are implementation details of PS and not relevant
        // to use. We use the same logic in the PartialEq impl, ignoring those
        // fields during comparisons. With those fields included in the Debug
        // impl, the assertion output from pretty_assertions identifies a lot
        // of false diff for failed assertions, making it hard to debug test
        // failures.
        if let Value::Function(Function::User { definition, .. }) = &self.0 {
            f.debug_struct("Procedure (dynamic)")
                .field("parameters", &definition.parameters)
                .field("body", &definition.body)
                .finish()
        } else {
            f.debug_tuple("Procedure").field(&self.0).finish()
        }
    }
}

impl Default for Procedure {
    fn default() -> Self {
        Self::new(Value::Undefined)
    }
}

impl Display for Procedure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for Procedure {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<Value> for Procedure {
    fn from(value: Value) -> Self {
        Self::new(value)
    }
}

impl From<Procedure> for Value {
    fn from(procedure: Procedure) -> Self {
        procedure.into_value()
    }
}
#[cfg(any(test, feature = "test"))]
impl From<i64> for Procedure {
    fn from(value: i64) -> Self {
        Self::new(value)
    }
}

#[cfg(any(test, feature = "test"))]
impl From<&str> for Procedure {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

#[cfg(any(test, feature = "test"))]
impl From<serde_json::Value> for Procedure {
    fn from(value: serde_json::Value) -> Self {
        // Convert JSON -> PS
        Self::new(serde_json::from_value::<Value>(value).unwrap())
    }
}

#[cfg(any(test, feature = "test"))]
impl PartialEq for Procedure {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            // When comparing user functions, we only care about the definition.
            // Trying to generate the correct capture set in tests is a giant
            // pain and doesn't accomplish much
            (
                Value::Function(Function::User {
                    definition: definition1,
                    ..
                }),
                Value::Function(Function::User {
                    definition: definition2,
                    ..
                }),
            ) => {
                // Ignore the function name and captures because they're
                // annoying and kinda pointless
                definition1.parameters == definition2.parameters
                    && definition1.body == definition2.body
            }
            // For anything else, defer to a normal structural comparison
            _ => self.0 == other.0,
        }
    }
}

/// A container for rendering a group of values. Create one renderer for each
/// recipe, so that state can be shared between related renders.
pub struct Renderer {
    /// The PetitScript process that will be used to call any dynamic render
    /// functions
    process: Process,
}

impl Renderer {
    /// Create a new renderer by forking a PS process. The given process should
    /// be the one that loaded the collection.
    pub fn new(process: Process, context: RenderContext) -> Self {
        // Create a new process for this renderer, so we can attach our
        // procedure context. All renders for a single recipe will share
        // the same context and state.
        let mut process = process.clone();
        // Setting app data can only fail if it's already set, which would
        // indicate a bug in our process handling
        process.set_app_data(context).unwrap();
        process.set_app_data(RenderState::default()).unwrap();
        Self { process }
    }

    /// Create a new renderer from an existing process that already has a
    /// render context attached. This should only be use for recursive renders
    /// from inside native functions, where the process has already been
    /// initialized for procedure rendering but you don't have access to the
    /// wrapping `Renderer`.
    pub fn forked(process: &Process) -> Self {
        Self {
            process: process.clone(),
        }
    }

    /// Get the [RenderContext] attached to this renderer
    pub fn context(&self) -> &RenderContext {
        // Context is only stored as app data in the process, so we don't have
        // to wrap it with an extra Arc. The repeated downcasting could
        // potentially be slower than the Arc, but it's simpler. This only
        // fails if the context isn't attached, which would be a bug in the
        // renderer setup.
        self.process.app_data().unwrap()
    }

    /// Render a procedure to a [petitscript::Value], then convert to a specific
    /// output type according to its [FromRendered] implementation.
    pub async fn render<T>(&self, procedure: &Procedure) -> anyhow::Result<T>
    where
        T: FromRendered,
    {
        let value = match &procedure.0 {
            // Function represents a rendering procedure - call it now
            Value::Function(function) => {
                self.render_function(function.clone()).await?
            }
            // A plain value can be returned directly
            other => other.clone(),
        };
        FromRendered::from_value(value)
    }

    /// Call a render function and return its value. Async native functions are
    /// implemented with an async-to-sync bridge that blocks, so rendering
    /// is pushed to a blocking task in tokio's blocking thread pool. We end up
    /// with an async-sync-async bridge which sucks, but it allows PetitScript
    /// to be entirely sync.
    async fn render_function(
        &self,
        function: Function,
    ) -> anyhow::Result<Value> {
        let process = self.process.clone();
        let return_value =
            task::spawn_blocking(move || process.call(&function, vec![]))
                .await??;
        Ok(return_value)
    }
}

/// Convert from a rendered [petitscript::Value] into `Self`. This abstraction
/// allows for other generic rendering code to handle multiple target types,
/// such as rendering to a string _or_ to bytes. Must be implement
/// `From<String>` for cases where the rendered value has been replaced by an
/// override string.
pub trait FromRendered: Sized + From<String> {
    fn from_value(value: Value) -> anyhow::Result<Self>;
}

impl FromRendered for Value {
    fn from_value(value: Value) -> anyhow::Result<Self> {
        Ok(value)
    }
}

impl FromRendered for String {
    fn from_value(value: Value) -> anyhow::Result<Self> {
        match value {
            Value::String(string) => Ok(string.into()),
            Value::Buffer(buffer) => {
                String::from_utf8(buffer.into()).map_err(anyhow::Error::from)
            }
            // Anything else should be stringified. We want the display string
            // here because it's more user-friendly
            other => Ok(format!("{other}")),
        }
    }
}

impl FromRendered for Bytes {
    fn from_value(value: Value) -> anyhow::Result<Self> {
        match value {
            Value::String(string) => Ok(String::from(string).into()),
            Value::Buffer(buffer) => Ok(buffer.into()),
            // Anything else should be stringified. We want the display string
            // here because it's more user-friendly
            other => Ok(format!("{other}").into()),
        }
    }
}

/// A little container struct for all the data needed to render dynamic
/// procedure functions. Unfortunately this has to own all data so templating
/// can be deferred into a task (tokio requires `'static` for spawned tasks).
/// This is exposed to native functions (such as `response`) via
/// [app_data](Process::app_data) on the PS process.
#[derive(Debug)]
pub struct RenderContext {
    /// Entire request collection
    pub collection: Arc<Collection>,
    /// ID of the profile whose data should be used for rendering. Generally
    /// the caller should check the ID is valid before passing it, to
    /// provide a better error to the user if not.
    pub selected_profile: Option<ProfileId>,
    /// An interface to allow accessing and sending HTTP chained requests
    pub http_provider: Box<dyn HttpProvider>,
    /// Additional key=value overrides passed directly from the user
    pub overrides: Overrides,
    /// A conduit to ask the user questions
    pub prompter: Box<dyn Prompter>,
    /// Should sensitive values be shown normally or masked? Enabled for
    /// request renders, disabled for previews
    pub show_sensitive: bool,
}

impl RenderContext {
    /// Get the selected profile
    pub fn profile(&self) -> Option<&Profile> {
        self.selected_profile
            .as_ref()
            .and_then(|profile_id| self.collection.profiles.get(profile_id))
    }
}

/// A set of fields whose values have been overridden at render time. Typically
/// the value for a recipe field is statically set in the collection, or
/// dynamically calculated via a function set in the recipe. Overrides allow
/// the user to modify values for a single request without modifying the
/// collection.
///
/// Override keys are used internally by the TUI and can be passed by the user
/// in the CLI with the `--override` flag.
pub type Overrides = IndexMap<OverrideKey, OverrideValue>;

/// A key specifying a single value in a request to be overridden. Users can
/// override a specific part of a recipe OR a profile field. Profile fields
/// provide more granular and customizable override behavior.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum OverrideKey {
    /// Override the value of a profile field
    Profile(String),
    /// Override the request URL
    Url,
    /// Override a single query parameter value. Query parameters can appear
    /// multiple times, so an additional index is used to disambiguate between
    /// multiple occurrences of the same param. The index will be `0` for the
    /// first appearance of *that parameter*, `1` for the second, etc.
    Query(String, usize),
    /// Override a single header value
    Header(String),
    /// Override the request's entire body. For raw/JSON bodies
    Body,
    /// Override a form body field
    Form(String),
    /// Override the username in basic authentication
    AuthenticationUsername,
    /// Override the password in basic authentication
    AuthenticationPassword,
    /// Override the token in bearer token authentication
    AuthenticationToken,
}

/// Parse an override key from a string source such as a CLI flag
impl FromStr for OverrideKey {
    type Err = OverrideKeyParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        /// Parse 1 or more characters
        fn any1(input: &mut &str) -> ModalResult<String> {
            take_while(1.., |c| c != '.')
                .map(String::from)
                .parse_next(input)
        }

        alt((
            "url".map(|_| Self::Url),
            "body".map(|_| Self::Body),
            // profile.<field>
            preceded("profile.", any1).map(Self::Profile),
            // query.<param> or query.<param>.<index>
            preceded("query.", (any1, opt(preceded(".", dec_uint))))
                .map(|(param, i)| Self::Query(param, i.unwrap_or(0))),
            preceded("headers.", any1).map(Self::Header),
            preceded("form.", any1).map(Self::Form),
            "auth.username".map(|_| Self::AuthenticationUsername),
            "auth.password".map(|_| Self::AuthenticationPassword),
            "auth.token".map(|_| Self::AuthenticationToken),
        ))
        .parse(s)
        .map_err(|error| OverrideKeyParseError(error.to_string()))
    }
}

/// An overridden recipe value. A value can be overriden either by providing a
/// new value, or by omitting it. Omitting a value drops it from the recipe.
/// Useful e.g. for disabling a query parameter.
#[derive(Debug, PartialEq)]
pub enum OverrideValue {
    Omit,
    Override(String),
}

impl From<&str> for OverrideValue {
    fn from(value: &str) -> Self {
        Self::Override(value.into())
    }
}

impl From<String> for OverrideValue {
    fn from(value: String) -> Self {
        Self::Override(value)
    }
}

/// State to be shared between multiple renders within a single render group
/// (i.e. a single recipe). This is attached as [app_data](Process::app_data)
/// on the process so it can be exposed to native PS functions.
#[derive(Debug, Default)]
pub struct RenderState {
    /// Multiple renders of the same profile field within the same recipe are
    /// cached, to prevent duplicate work (e.g. running the same prompt twice).
    /// The error must be in an `Arc` so we can share failures as well.
    pub(crate) profile_cache:
        FutureCache<String, Result<Value, Arc<anyhow::Error>>>,
}

#[cfg(any(test, feature = "test"))]
impl slumber_util::Factory for RenderContext {
    fn factory(_: ()) -> Self {
        use crate::{
            database::CollectionDatabase,
            test_util::{TestHttpProvider, TestPrompter},
        };
        Self {
            collection: Default::default(),
            selected_profile: None,
            http_provider: Box::new(TestHttpProvider::new(
                CollectionDatabase::factory(()),
                None,
            )),
            overrides: Default::default(),
            prompter: Box::<TestPrompter>::default(),
            show_sensitive: true,
        }
    }
}

/// An abstraction that provides behavior for chained HTTP requests. This
/// enables fetching past requests and sending requests. The implementor is
/// responsible for providing the data store of the requests, and persisting
/// the sent request as appropriate.
#[async_trait] // Native async fn isn't dyn-compatible
pub trait HttpProvider: Debug + Send + Sync {
    /// Get the most recent request for a particular profile+recipe
    async fn get_latest_request(
        &self,
        profile_id: Option<&ProfileId>,
        recipe_id: &RecipeId,
    ) -> anyhow::Result<Option<Exchange>>;

    /// Build and send an HTTP request. The implementor may choose whether
    /// triggered chained requests will be sent, and whether the result should
    /// be persisted in the database.
    async fn send_request(
        &self,
        seed: RequestSeed,
        renderer: &Renderer,
    ) -> Result<Exchange, TriggeredRequestError>;
}

/// A prompter is a bridge between the user and the render engine. It enables
/// the render engine to request values from the user *during* the render
/// process. The implementor is responsible for deciding *how* to ask the user.
///
/// **Note:** The prompter has to be able to handle simultaneous prompt
/// requests, such as if a procedure has multiple prompt values, or if multiple
/// procedures with prompts are being rendered simultaneously.  The implementor
/// is responsible for queueing prompts to show to the user one at a time.
pub trait Prompter: Debug + Send + Sync {
    /// Ask the user a question, and use the given channel to return a response.
    /// To indicate "no response", simply drop the returner.
    ///
    /// If an error occurs while prompting the user, just drop the returner.
    /// The implementor is responsible for logging the error as appropriate.
    fn prompt(&self, prompt: Prompt);

    /// Ask the user to pick an item for a list of choices
    fn select(&self, select: Select);
}

/// Data defining a prompt which should be presented to the user
#[derive(Debug)]
pub struct Prompt {
    /// Tell the user what we're asking for
    pub message: String,
    /// Value used to pre-populate the text box
    pub default: Option<String>,
    /// Should the value the user is typing be masked? E.g. password input
    pub sensitive: bool,
    /// How the prompter will pass the answer back
    pub channel: ResponseChannel<String>,
}

/// A list of options to present to the user
#[derive(Debug)]
pub struct Select {
    /// Tell the user what we're asking for
    pub message: String,
    /// List of choices the user can pick from
    pub options: Vec<String>,
    /// How the prompter will pass the answer back
    pub channel: ResponseChannel<String>,
}

/// Channel used to return a response to a one-time request. This is its own
/// type so we can provide wrapping functionality
#[derive(Debug, From)]
pub struct ResponseChannel<T>(oneshot::Sender<T>);

impl<T> ResponseChannel<T> {
    /// Return the value that the user gave
    pub fn respond(self, response: T) {
        // This error *shouldn't* ever happen, because the templating task
        // stays open until it gets a response
        let _ = self
            .0
            .send(response)
            .map_err(|_| anyhow!("Response listener dropped"))
            .traced();
    }
}

/// Error for [OverrideKey]'s `FromStr` impl.
#[derive(Debug, Error)]
#[error("Invalid override key: {0}")]
pub struct OverrideKeyParseError(String);

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;
    use slumber_util::assert_err;

    #[rstest]
    #[case::url("url", OverrideKey::Url)]
    #[case::profile("profile.field", OverrideKey::Profile("field".into()))]
    #[case::query("query.param", OverrideKey::Query("param".into(), 0))]
    #[case::query_index("query.param.1", OverrideKey::Query("param".into(), 1))]
    #[case::header("headers.field", OverrideKey::Header("field".into()))]
    #[case::form("form.field", OverrideKey::Form("field".into()))]
    #[case::body("body", OverrideKey::Body)]
    #[case::authentication_username(
        "auth.username",
        OverrideKey::AuthenticationUsername
    )]
    #[case::authentication_password(
        "auth.password",
        OverrideKey::AuthenticationPassword
    )]
    #[case::authentication_token(
        "auth.token",
        OverrideKey::AuthenticationToken
    )]
    fn test_parse_override_key(
        #[case] input: &str,
        #[case] expected_key: OverrideKey,
    ) {
        let parsed: OverrideKey = input.parse().unwrap();
        assert_eq!(parsed, expected_key);
    }

    /// Test parsing invalid override keys
    #[rstest]
    #[case::empty("")]
    #[case::empty_field("profile.")]
    #[case::trailing_dot("profile.field.")]
    #[case::empty_invalid_query_index("query.p1.w")]
    fn test_parse_override_key_error(#[case] input: &str) {
        assert_err!(input.parse::<OverrideKey>(), "Invalid override key");
    }
}
