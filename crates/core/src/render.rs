//! Template rendering tools. This is a wrapper around the template engine from
//! [slumber_template], with context and functions specific to rendering HTTP
//! requests.

use crate::{
    collection::{Collection, ProfileId, RecipeId},
    http::{Exchange, RequestSeed, TriggeredRequestError},
};
use anyhow::anyhow;
use async_trait::async_trait;
use derive_more::From;
use indexmap::IndexMap;
use serde_json_path::JsonPath;
use slumber_template::TemplateEngine;
use slumber_util::ResultTraced;
use std::{
    fmt::Debug,
    sync::{Arc, LazyLock},
};
use tokio::{fs, sync::oneshot};

/// Shared template engine for the entire process. The template engine is
/// stateless and immutable after initialization, so we can share one instance
/// everywhere.
static TEMPLATE_ENGINE: LazyLock<TemplateEngine<TemplateContext>> =
    LazyLock::new(|| {
        // TODO the rest of these
        // exports.export_fn("command", sync(command));
        // exports.export_fn("response", sync(response));
        // exports.export_fn("responseHeader", sync(response_header));
        // exports.export_fn("select", sync(select));

        let mut engine = TemplateEngine::new();
        // Filters
        engine.add_filter("jsonpath", jsonpath);
        engine.add_filter("sensitive", sensitive);

        // Producers
        engine.add_producer("file", file);
        engine.add_producer("prompt", prompt);
        engine
    });

/// Get a reference to the global template engine. This should be used for all
/// renders.
pub fn engine() -> &'static TemplateEngine<TemplateContext> {
    &*TEMPLATE_ENGINE
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
    pub overrides: IndexMap<String, String>,
    /// A conduit to ask the user questions
    pub prompter: Box<dyn Prompter>,
    /// TODO
    pub show_sensitive: bool,
}

impl slumber_template::TemplateContext for TemplateContext {
    async fn get(
        &self,
        identifier: &slumber_template::Identifier,
        engine: &TemplateEngine<Self>,
    ) -> Result<slumber_template::Value, slumber_template::TemplateError> {
        todo!()
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
        template_context: &TemplateContext,
    ) -> Result<Exchange, TriggeredRequestError>;
}

/// A prompter is a bridge between the user and the template engine. It enables
/// the template engine to request values from the user *during* the template
/// process. The implementor is responsible for deciding *how* to ask the user.
///
/// **Note:** The prompter has to be able to handle simultaneous prompt
/// requests, if a template has multiple prompt values, or if multiple templates
/// with prompts are being rendered simultaneously.  The implementor is
/// responsible for queueing prompts to show to the user one at a time.
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

/// TODO
async fn file(path: &str) -> Vec<u8> {
    fs::read(path).await
}

/// Transform a JSON value using a JSONPath query
fn jsonpath(
    ViaDeserialize(query): ViaDeserialize<JsonPath>,
    ViaDeserialize(value): ViaDeserialize<serde_json::Value>,
) -> Result<serde_json::Value, Error> {
    // TODO support mode?
    let node_list = query.query(&value);
    // TODO can we avoid this collection? is it possible to go NodeList straight
    // to Value? what serializer/deserialize do we use?
    let json: serde_json::Value = node_list.into_iter().cloned().collect();
    // Convert from JSON to minijinja::Value
    Ok(serde_json::from_value(json).unwrap())
}

/// TODO
async fn prompt(
    context: &TemplateContext,
    kwargs: Kwargs,
) -> Result<String, Error> {
    // TODO static kwargs
    let message: Option<String> = kwargs.get("message")?;
    let default: Option<String> = kwargs.get("default")?;
    let sensitive: bool =
        kwargs.get::<Option<bool>>("sensitive")?.unwrap_or(false);
    let (tx, rx) = oneshot::channel();
    context.prompter.prompt(Prompt {
        message: message.unwrap_or_default(),
        default,
        sensitive,
        channel: tx.into(),
    });
    let output = rx.await.map_err(|_| FunctionError::PromptNoReply)?;

    // If the input was sensitive, we should mask it for previews as well.
    // This is a little wonky because the preview prompter just spits out a
    // static string anyway, but it's "technically" right and plays well in
    // tests. Also it reminds users that a prompt is sensitive in the TUI :)
    if sensitive {
        Ok(mask_sensitive(&context, output))
    } else {
        Ok(output)
    }
}

/// TODO
fn sensitive(context: &TemplateContext, value: String) -> String {
    mask_sensitive(&context, value)
}

/// Hide a sensitive value if the context has show_sensitive disabled
fn mask_sensitive(context: &TemplateContext, value: String) -> String {
    if context.show_sensitive {
        value
    } else {
        "•".repeat(value.chars().count())
    }
}
