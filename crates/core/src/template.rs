//! Generate strings (and bytes) from user-written templates with dynamic data
//! TODO update comment

// TODO remove old modules
mod cereal;
mod error;
// mod parse;
mod prompt;
// mod render;
mod functions;
#[cfg(test)]
mod tests;

pub use error::{ChainError, TemplateError, TriggeredRequestError};
pub use prompt::{Prompt, Prompter, ResponseChannel, Select};

use crate::{
    collection::{Collection, ProfileId, RecipeId},
    http::{Exchange, RequestSeed},
};
use async_trait::async_trait;
use derive_more::{Deref, Display};
use indexmap::IndexMap;
use minijinja::{
    Environment, UndefinedBehavior, Value,
    value::{DynObject, Object, ObjectRepr},
};
use serde::{Deserialize, Serialize};
use std::{fmt::Debug, io, sync::Arc};
use tokio::task;
use uuid::Uuid;

/// A parsed template, which can contain raw and/or templated content. The
/// string is parsed during creation to identify template keys, hence the
/// immutability.
///
/// The original string is *not* stored. To recover the source string, use the
/// [Display] implementation.
///
/// Invariants:
/// - Two templates with the same source string will have the same set of
///   chunks, and vice versa
/// - No two raw segments will ever be consecutive
#[derive(Clone, Debug, PartialEq)]
pub struct Template {
    id: Uuid,
}

impl Template {
    /// Create a new template from a raw string, without parsing it at all.
    /// Useful when importing from external formats where the string isn't
    /// expected to be a valid Slumber template
    pub fn raw(template: String) -> Template {
        todo!()
    }

    /// Render the template using values from the given context. If any chunk
    /// failed to render, return an error. The rendered template will be
    /// converted from raw bytes to UTF-8. If it is not valid UTF-8, return an
    /// error.
    /// TODO update comment
    pub async fn render_string(
        &self,
        context: TemplateContext,
    ) -> Result<String, TemplateError> {
        let environment = Arc::clone(&context.environment);
        let template_id = self.id;
        let context = Value::from_object(context);
        task::spawn_blocking(move || {
            let template =
                environment.get_template(&template_id.to_string())?;
            template.render(context)
        })
        .await
        .expect("TODO")
        .map_err(TemplateError::from)
    }

    /// TODO
    pub async fn render_bytes(
        &self,
        context: TemplateContext,
    ) -> Result<Vec<u8>, TemplateError> {
        // TODO render to bytes directly
        self.render_string(context).await.map(String::into_bytes)
    }
}

/// A little container struct for all the data that the user can access via
/// templating. Unfortunately this has to own all data so templating can be
/// deferred into a task (tokio requires `'static` for spawned tasks). If this
/// becomes a bottleneck, we can `Arc` some stuff.
///
/// TODO remove clone?
#[derive(Clone, Debug)]
pub struct TemplateContext {
    /// TODO
    pub environment: Arc<Environment<'static>>,
    /// Entire request collection
    pub collection: Arc<Collection>,
    /// ID of the profile whose data should be used for rendering. Generally
    /// the caller should check the ID is valid before passing it, to
    /// provide a better error to the user if not.
    pub selected_profile: Option<ProfileId>,
    /// An interface to allow accessing and sending HTTP chained requests
    pub http_provider: Arc<dyn HttpProvider>,
    /// Additional key=value overrides passed directly from the user
    pub overrides: IndexMap<String, String>,
    /// A conduit to ask the user questions
    pub prompter: Arc<dyn Prompter>,
    /// Should sensitive values be shown normally or masked? Enabled for
    /// request renders, disabled for previews
    pub show_sensitive: bool,
}

impl TemplateContext {
    /// The canonical way to access context through state is to use a special
    /// key for it
    /// https://github.com/mitsuhiko/minijinja/issues/796#issuecomment-2889925084
    const SELF_KEY: &str = "$context";
}

impl Object for TemplateContext {
    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        let key = key.as_str()?;
        match key {
            Self::SELF_KEY => {
                // Return ourselves in a new object
                Some(Value::from_dyn_object(DynObject::new(Arc::clone(self))))
            }
            _ => {
                let profile = self
                    .collection
                    .profiles
                    .get(self.selected_profile.as_ref()?)?;
                // TODO nested renders
                let template = profile.data.get(key)?;
                let template_todo = self
                    .environment
                    .get_template(&template.id.to_string())
                    .ok()?;
                Some(template_todo.source().into())
            }
        }
    }

    fn repr(self: &Arc<Self>) -> ObjectRepr {
        ObjectRepr::Plain
    }
}

#[cfg(any(test, feature = "test"))]
impl slumber_util::Factory for TemplateContext {
    fn factory(_: ()) -> Self {
        use crate::{
            database::CollectionDatabase,
            test_util::{TestHttpProvider, TestPrompter},
        };
        Self {
            environment: Default::default(),
            collection: Default::default(),
            selected_profile: None,
            http_provider: Arc::new(TestHttpProvider::new(
                CollectionDatabase::factory(()),
                None,
            )),
            overrides: IndexMap::new(),
            prompter: Arc::<TestPrompter>::default(),
            show_sensitive: true,
        }
    }
}

/// An identifier that can be used in a template key. A valid identifier is
/// any non-empty string that contains only alphanumeric characters, `-`, or
/// `_`.
///
/// Construct via [FromStr](std::str::FromStr)
#[derive(
    Clone,
    Debug,
    Deref,
    Default,
    Display,
    Eq,
    Hash,
    PartialEq,
    Serialize,
    Deserialize,
)]
#[serde(transparent)]
#[cfg_attr(test, derive(proptest_derive::Arbitrary))]
pub struct Identifier(
    #[cfg_attr(test, proptest(regex = "[a-zA-Z0-9-_]+"))] String,
);

/// A shortcut for creating identifiers from static strings. Since the string
/// is defined in code we're assuming it's valid.
impl From<&'static str> for Identifier {
    fn from(value: &'static str) -> Self {
        Self(value.parse().unwrap())
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
        template_context: TemplateContext,
    ) -> Result<Exchange, TriggeredRequestError>;
}

/// TODO
pub(crate) fn new_environment() -> Environment<'static> {
    let mut environment = Environment::new();
    environment.set_undefined_behavior(UndefinedBehavior::Strict);
    functions::add_functions(&mut environment);
    environment
}
