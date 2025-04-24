//! Render PetitScript recipe values into static strings and bytes for HTTP
//! requests.

use crate::{
    collection::{Collection, Profile, ProfileId, RecipeId},
    http::{Exchange, RequestBuildError, RequestError, RequestSeed},
    util::FutureCache,
};
use anyhow::anyhow;
use async_trait::async_trait;
use bytes::Bytes;
use derive_more::{Display, From};
use indexmap::IndexMap;
use petitscript::{Process, Value, value::Function};
use serde::{Deserialize, Serialize};
use slumber_util::ResultTraced;
use std::{borrow::Cow, fmt::Debug, str::FromStr, sync::Arc};
use thiserror::Error;
use tokio::{sync::oneshot, task};

/// A definition of a how a recipe value should be rendered.
/// [petitscript::Value] to be used in a recipe. Procedures come in two forms:
/// - Static: a predefined value such as a number, string, or object
/// - Dynamic: a function that dynamically generates a value at render time,
///   based on external factors such as files, responses, or user input
///
/// The name "procedure" is fairly arbitrary, but it's specific and unique so
/// it works. This is the successor to Slumber's previous "template" system,
/// which were declarative strings.
#[derive(Clone, Debug, Default, Display, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Procedure(Value);

impl Procedure {
    pub fn new(value: impl Into<Value>) -> Self {
        Self(value.into())
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

#[cfg(any(test, feature = "test"))]
impl From<&str> for Procedure {
    fn from(_: &str) -> Self {
        todo!("get rid of this?")
    }
}

impl From<String> for Procedure {
    fn from(value: String) -> Self {
        Procedure(value.into())
    }
}

#[cfg(any(test, feature = "test"))]
impl From<serde_json::Value> for Procedure {
    fn from(value: serde_json::Value) -> Self {
        format!("{value:#}").into()
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
        // TODO error context here?
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
pub type Overrides = IndexMap<OverrideKey<'static>, OverrideValue>;

/// A key specifying a single value in a request to be overridden. Users can
/// override a specific part of a recipe OR a profile field. Profile fields
/// provide more granular and customizable override behavior.
///
/// `Cow` is used here to prevent unnecessary cloning when checking for keys in
/// an override map.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum OverrideKey<'a> {
    /// Override the value of a profile field
    Profile(Cow<'a, str>),
    /// Override the request URL
    Url,
    /// Override a single query parameter value
    Query(Cow<'a, str>),
    /// Override a single header value
    Header(Cow<'a, str>),
    /// Override the request's entire body. For raw/JSON bodies
    Body,
    /// Override a form body field
    Form(Cow<'a, str>),
    /// Override the username in basic authentication
    AuthenticationUsername,
    /// Override the password in basic authentication
    AuthenticationPassword,
    /// Override the token in bearer token authentication
    AuthenticationToken,
}

impl FromStr for OverrideKey<'static> {
    type Err = OverrideKeyParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match (s, s.split_once('.')) {
            ("url", _) => Ok(Self::Url),
            ("body", _) => Ok(Self::Body),
            (_, Some(("profile", field))) => {
                Ok(Self::Profile(field.to_owned().into()))
            }
            (_, Some(("query", param))) => {
                Ok(Self::Query(param.to_owned().into()))
            }
            (_, Some(("headers", name))) => {
                Ok(Self::Header(name.to_owned().into()))
            }
            _ => Err(OverrideKeyParseError),
        }
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
#[error("Invalid override key")]
pub struct OverrideKeyParseError;

/// Error occurred while trying to build/execute a triggered request.
///
/// This type implements `Clone` so it can be shared between deduplicated chain
/// renders, hence the `Arc`s on inner errors.
///
/// TODO move this to http or ps::error
#[derive(Clone, Debug, Error)]
#[cfg_attr(test, derive(PartialEq))]
pub enum TriggeredRequestError {
    /// This render was invoked in a way that doesn't support automatic request
    /// execution. In some cases the user needs to explicitly opt in to enable
    /// it (e.g. with a CLI flag)
    #[error("Triggered request execution not allowed in this context")]
    NotAllowed,

    /// Tried to auto-execute a chained request but couldn't build it
    #[error(transparent)]
    Build(#[from] Arc<RequestBuildError>),

    /// Chained request was triggered, sent and failed
    #[error(transparent)]
    Send(#[from] Arc<RequestError>),
}

impl From<RequestBuildError> for TriggeredRequestError {
    fn from(error: RequestBuildError) -> Self {
        Self::Build(error.into())
    }
}

impl From<RequestError> for TriggeredRequestError {
    fn from(error: RequestError) -> Self {
        Self::Send(error.into())
    }
}
