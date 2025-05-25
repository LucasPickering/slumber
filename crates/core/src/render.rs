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
use itertools::Itertools;
use serde::{Deserialize, de::value::SeqDeserializer};
use serde_json_path::JsonPath;
use slumber_template::{TemplateEngine, TemplateError, ViaSerde};
use slumber_util::ResultTraced;
use std::{
    env,
    fmt::Debug,
    io, iter,
    path::PathBuf,
    sync::{Arc, LazyLock},
};
use thiserror::Error;
use tokio::{fs, sync::oneshot};

/// Shared template engine for the entire process. The template engine is
/// stateless and immutable after initialization, so we can share one instance
/// everywhere.
static TEMPLATE_ENGINE: LazyLock<TemplateEngine<TemplateContext>> =
    LazyLock::new(|| {
        let mut engine = TemplateEngine::new();

        // TODO remove need for type annotations
        // Register all functions
        // engine.add_function("command", command);
        engine.add_function::<_, (String,), _>("env", env);
        // engine.add_function("file", file);
        engine.add_function::<_, (ViaSerde<_>, ViaSerde<_>), _>(
            "jsonpath", jsonpath,
        );
        // engine.add_function("prompt", prompt);
        // engine.add_function("response", response);
        // engine.add_function("responseHeader", response_header);
        // engine.add_function("select", select);
        engine.add_function::<_, (&TemplateContext, String), _>(
            "sensitive",
            sensitive,
        );
        engine
    });

/// Get a reference to the global template engine. This should be used for all
/// renders.
pub fn engine() -> &'static TemplateEngine<TemplateContext> {
    &TEMPLATE_ENGINE
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

/// Get the value of an environment variable. Return `None` if the variable is
/// not set
fn env(variable: String) -> Option<String> {
    env::var(variable).ok()
}

/*

/// TODO
async fn file(path: &str) -> Vec<u8> {
    fs::read(path).await
}
*/

/// Transform a JSON value using a JSONPath query
fn jsonpath(
    // Value first so it can be piped in
    ViaSerde(value): ViaSerde<serde_json::Value>,
    ViaSerde(query): ViaSerde<JsonPath>,
) -> Result<slumber_template::Value, FunctionError> {
    // TODO support mode?
    let node_list = query.query(&value);
    // Deserialize from the JSON list into a template value. This should be
    // infallible because template values are a superset of JSON
    slumber_template::Value::deserialize(SeqDeserializer::new(
        node_list.into_iter(),
    ))
    .map_err(|_| todo!())
}

/*
/// TODO
async fn prompt(
    context: &TemplateContext,
    kwargs: Kwargs,
) -> Result<String, FunctionError> {
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
*/

/// Hide a sensitive value if the context has show_sensitive disabled
fn sensitive(context: &TemplateContext, value: String) -> String {
    if context.show_sensitive {
        value
    } else {
        "•".repeat(value.chars().count())
    }
}

/// TODO
#[derive(Debug, Error)]
pub enum FunctionError {
    /// Error executing an external command
    #[error(
        "Executing command `{}`", iter::once(program).chain(args).format(" ")
    )]
    Command {
        program: String,
        args: Vec<String>,
        #[source]
        error: io::Error,
    },

    /// User passed an empty command arrary
    #[error("Command must have at least one element")]
    CommandEmpty,

    /// An error occurred accessing the persistence database. This error is
    /// generated by our code so we don't need any extra context.
    #[error(transparent)]
    Database(anyhow::Error),

    /// Error opening/reading a file
    #[error("Reading file `{path}`")]
    File {
        path: PathBuf,
        #[source]
        error: io::Error,
    },

    /// Error decoding bytes as UTF-8
    #[error(transparent)]
    InvalidUtf8(#[from] std::string::FromUtf8Error),

    /// JSONPath query returned 0 or 2+ results when we expected 1
    #[error(
        "Expected exactly one result from JSONPath query `{query}`, \
        but got {actual_count}"
    )]
    JsonPathExactlyOne {
        query: JsonPath,
        actual_count: usize,
    },

    /// JSONPath query returned no results when it should have
    #[error("No results from JSONPath query `{query}`")]
    JsonPathNoResults { query: JsonPath },

    /// Render context not available. Could be a bug, but it probably indicates
    /// a render function was called outside a render
    #[error(
        "Render context not available. Slumber render functions can only \
        be called during a recipe render. For recipe fields, make sure the \
        template string is a function: () => `My name is ${{username()}}`"
    )]
    NoContext,

    /// An bubbled-up error from rendering a profile field value
    #[error("Nested render for field `{field}`")]
    ProfileNested {
        field: String,
        /// The bubbled error. This needs an `Arc` because profile render
        /// results are cached, therefore the error must be `Clone`.
        #[source]
        error: Arc<anyhow::Error>,
    },

    /// Never got a reply from the prompt channel. Do *not* store the
    /// `RecvError` here, because it provides useless extra output to the user.
    #[error("No reply from prompt/select")]
    PromptNoReply,

    /// Recipe for `response()` has no history
    #[error("No response available")]
    ResponseMissing,

    /// Specified header did not exist in the response
    #[error("Header `{header}` not in response")]
    ResponseMissingHeader { header: String },

    /// Something bad happened while triggering a request dependency
    #[error("Triggering upstream recipe `{recipe_id}`")]
    Trigger {
        recipe_id: RecipeId,
        #[source]
        error: TriggeredRequestError,
    },
}

impl From<FunctionError> for TemplateError {
    fn from(error: FunctionError) -> Self {
        TemplateError::other(error)
    }
}
