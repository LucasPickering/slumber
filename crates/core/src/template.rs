//! Generate strings (and bytes) from user-written templates with dynamic data

mod error;
mod prompt;
#[cfg(test)]
mod tests;

pub use error::{TemplateError, TriggeredRequestError};
pub use prompt::{Prompt, Prompter, ResponseChannel, Select};

use crate::{
    collection::{Collection, Profile, ProfileId, RecipeId},
    http::{Exchange, RequestSeed},
    template::error::OverrideKeyParseError,
    util::FutureCache,
};
use async_trait::async_trait;
use bytes::Bytes;
use derive_more::Display;
use indexmap::IndexMap;
use petitscript::{Process, Value, function::Function};
use serde::{Deserialize, Serialize};
use std::{borrow::Cow, fmt::Debug, str::FromStr, sync::Arc};
use tokio::task;

/// TODO
#[derive(Clone, Debug, Default, Display, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Template(Value);

impl Template {
    pub fn new(value: impl Into<Value>) -> Self {
        Self(value.into())
    }

    /// TODO
    pub fn is_dynamic(&self) -> bool {
        matches!(&self.0, Value::Function(_))
    }
}

#[cfg(any(test, feature = "test"))]
impl From<&str> for Template {
    fn from(_: &str) -> Self {
        todo!("get rid of this?")
    }
}

impl From<String> for Template {
    fn from(value: String) -> Self {
        Template(value.into())
    }
}

#[cfg(any(test, feature = "test"))]
impl From<serde_json::Value> for Template {
    fn from(value: serde_json::Value) -> Self {
        format!("{value:#}").into()
    }
}

/// A container for rendering a group of values. Create one renderer for each
/// recipe, so that state can be shared between related renders.
pub struct Renderer {
    process: Process,
}

impl Renderer {
    /// TODO
    pub fn new(process: Process, context: TemplateContext) -> Self {
        // Create a new process for this renderer, so we can attach our template
        // context. All renders for a single recipe will share the context
        let mut process = process.clone();
        process.set_app_data(context).expect("TODO");
        // State that may be shared between renders of this group
        process.set_app_data(RenderState::default()).expect("TODO");
        Self { process }
    }

    /// Create a new renderer from an existing process that already has a
    /// template context attached. This should only be use for recursive renders
    /// from inside native functions, where the process has already been
    /// initialized for template rendering but you don't have access to the
    /// wrapping `Renderer`.
    pub fn forked(process: &Process) -> Self {
        Self {
            process: process.clone(),
        }
    }

    /// Get the [TemplateContext] attached to this renderer
    pub fn context(&self) -> &TemplateContext {
        // Context is only stored as app data in the process, so we don't have
        // to wrap it with an extra Arc. The repeated downcasting could
        // potentially be slower than the Arc, but it's simpler
        self.process.app_data().expect("TODO")
    }

    /// Render a template to a [petitscript::Value], then convert to a specific
    /// output type according to its [FromRendered] implementation.
    pub async fn render<T>(&self, template: &Template) -> anyhow::Result<T>
    where
        T: FromRendered,
    {
        let value = match &template.0 {
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
            task::spawn_blocking(move || process.call(&function, &[]))
                .await??;
        Ok(return_value)
    }
}

/// Convert from a rendered [petitscript::Value] into `Self`. This abstraction
/// allows for other generic rendering code to handle multiple target types.
/// Must be convertible from `String` for cases where the rendered value has
/// been replaced by an override string.
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
            // Anything else should be stringified
            other => Ok(other.to_string()),
        }
    }
}

impl FromRendered for Bytes {
    fn from_value(value: Value) -> anyhow::Result<Self> {
        match value {
            Value::String(string) => Ok(String::from(string).into()),
            Value::Buffer(buffer) => Ok(buffer.into()),
            // Anything else should be stringified
            other => Ok(other.to_string().into()),
        }
    }
}

/// A little container struct for all the data that the user can access via
/// templating. Unfortunately this has to own all data so templating can be
/// deferred into a task (tokio requires `'static` for spawned tasks). If this
/// becomes a bottleneck, we can `Arc` some stuff.
#[derive(Debug)]
pub struct TemplateContext {
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
}

impl TemplateContext {
    /// TODO
    pub fn profile(&self) -> Option<&Profile> {
        self.selected_profile
            .as_ref()
            .and_then(|profile_id| self.collection.profiles.get(profile_id))
    }
}

/// TODO
pub type Overrides = IndexMap<OverrideKey<'static>, OverrideValue>;

/// A key specifying a single value in a request to be overridden. Users can
/// override a specific part of a recipe OR a profile field. Profile fields
/// provide more granular and customizable override behavior.
///
/// Override keys are used internally by the TUI, and can be passed by the user
/// in the CLI with the `--override` flag.
///
/// TODO explain or remove Cow
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

/// TODO
#[derive(Debug, PartialEq)]
pub enum OverrideValue {
    Omit,
    Override(String),
}

/// TODO
#[derive(Debug, Default)]
pub struct RenderState {
    /// Multiple renders of the same profile field within the same recipe are
    /// cached, to prevent duplicate work (e.g. running the same prompt twice).
    /// The error must be in an `Arc` so we can share failures as well.
    pub(crate) profile_cache:
        FutureCache<String, Result<Value, Arc<anyhow::Error>>>,
}

#[cfg(any(test, feature = "test"))]
impl slumber_util::Factory for TemplateContext {
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

/// Join consecutive raw chunks in a generated template, to make it valid
#[cfg(test)]
fn join_raw(chunks: Vec<TemplateInputChunk>) -> Vec<TemplateInputChunk> {
    let len = chunks.len();
    chunks
        .into_iter()
        .fold(Vec::with_capacity(len), |mut chunks, chunk| {
            match (chunks.last_mut(), chunk) {
                // If previous and current are both raw, join them together
                (
                    Some(TemplateInputChunk::Raw(previous)),
                    TemplateInputChunk::Raw(current),
                ) => {
                    // The current string is inside an Arc so we can't push
                    // into it, we have to clone it out :(
                    let mut concat =
                        String::with_capacity(previous.len() + current.len());
                    concat.push_str(previous);
                    concat.push_str(&current);
                    *previous = Arc::new(concat)
                }
                (_, chunk) => chunks.push(chunk),
            }
            chunks
        })
}
