//! Template rendering tools. This is a wrapper around the template engine from
//! [slumber_template], with context and functions specific to rendering HTTP
//! requests.

mod functions;

use crate::{
    collection::{Collection, Profile, ProfileId, RecipeId},
    http::{Exchange, RequestSeed, ResponseRecord, TriggeredRequestError},
    render::functions::RequestTrigger,
};
use anyhow::anyhow;
use async_trait::async_trait;
use chrono::Utc;
use derive_more::From;
use indexmap::IndexMap;
use itertools::Itertools;
use serde_json_path::JsonPath;
use slumber_template::{Arguments, Identifier, TemplateError};
use slumber_util::ResultTraced;
use std::{fmt::Debug, io, iter, path::PathBuf, sync::Arc};
use thiserror::Error;
use tokio::sync::oneshot;

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
    /// Should sensitive values be shown normally or masked? Enabled for
    /// request renders, disabled for previews
    pub show_sensitive: bool,
}

impl TemplateContext {
    fn current_profile(&self) -> Option<&Profile> {
        self.selected_profile
            .as_ref()
            .and_then(|id| self.collection.profiles.get(id))
    }

    /// Get the most recent response for a profile+recipe pair. This will
    /// trigger the request if it is expired, and await the response
    async fn get_latest_response(
        &self,
        recipe_id: &RecipeId,
        trigger: RequestTrigger,
    ) -> Result<Arc<ResponseRecord>, FunctionError> {
        // Defer loading the most recent exchange until we know we'll need it
        let get_latest = || async {
            self.http_provider
                .get_latest_request(self.selected_profile.as_ref(), recipe_id)
                .await
                .map_err(FunctionError::Database)
        };

        // Helper to execute the request, if triggered
        let send_request = || async {
            // There are 3 different ways we can generate the build optoins:
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

            self.http_provider
                .send_request(
                    RequestSeed::new(recipe_id.clone(), build_options),
                    self,
                )
                .await
                .map_err(|error| FunctionError::Trigger {
                    recipe_id: recipe_id.clone(),
                    error,
                })
        };

        let exchange = match trigger {
            RequestTrigger::Never => {
                get_latest().await?.ok_or(FunctionError::ResponseMissing)?
            }
            RequestTrigger::NoHistory => {
                // If a exchange is present in history, use that. If not, fetch
                if let Some(exchange) = get_latest().await? {
                    exchange
                } else {
                    send_request().await?
                }
            }
            RequestTrigger::Expire { duration } => match get_latest().await? {
                Some(exchange)
                    if exchange.end_time + duration.inner() >= Utc::now() =>
                {
                    exchange
                }
                _ => send_request().await?,
            },
            RequestTrigger::Always => send_request().await?,
        };

        Ok(exchange.response)
    }
}

impl slumber_template::TemplateContext for TemplateContext {
    async fn get(
        &self,
        field: &slumber_template::Identifier,
    ) -> Result<slumber_template::Value, slumber_template::TemplateError> {
        // Check overrides first. The override value is NOT treated as a
        // template
        if let Some(value) = self.overrides.get(field.as_str()) {
            return Ok(value.clone().into());
        }

        // Then check the current profile
        let template = self
            .current_profile()
            .and_then(|profile| profile.data.get(field.as_str()))
            .ok_or_else(|| FunctionError::UnknownField {
                field: field.to_string(),
            })?;

        // Render the nested template
        let bytes = template.render_bytes(self).await?;
        Ok(bytes.into())
    }

    async fn call(
        &self,
        function_name: &Identifier,
        arguments: Arguments<'_, Self>,
    ) -> Result<slumber_template::Value, TemplateError> {
        match function_name.as_str() {
            "command" => functions::command(arguments).await,
            "env" => functions::env(arguments),
            "file" => functions::file(arguments).await,
            "jsonpath" => functions::jsonpath(arguments),
            "prompt" => functions::prompt(arguments).await,
            "response" => functions::response(arguments).await,
            "response_header" => functions::response_header(arguments).await,
            "select" => functions::select(arguments).await,
            "sensitive" => functions::sensitive(arguments),
            "trim" => functions::trim(arguments),
            _ => Err(TemplateError::UnknownFunction {
                name: function_name.clone(),
            }),
        }
    }
}

#[cfg(any(test, feature = "test"))]
impl slumber_util::Factory for TemplateContext {
    fn factory((): ()) -> Self {
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
            overrides: IndexMap::new(),
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

/// An error that can occur within a template function
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

    /// User referenced a field that isn't defined in the current profile
    #[error("Unknown profile field `{field}`")]
    UnknownField { field: String },
}

impl From<FunctionError> for TemplateError {
    fn from(error: FunctionError) -> Self {
        TemplateError::other(error)
    }
}
