//! Template rendering tools. This is a wrapper around the template engine from
//! [slumber_template], with context and functions specific to rendering HTTP
//! requests.

mod functions;
#[cfg(test)]
mod tests;

#[cfg(any(test, feature = "test"))]
use crate::collection::Recipe;
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
use slumber_template::{
    Arguments, FieldCache, Identifier, RenderError, Stream,
};
use slumber_util::ResultTraced;
use std::{
    fmt::Debug, io, iter, path::PathBuf, process::ExitStatus, sync::Arc,
};
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
    /// Directory in which to run file system operations. Should be the
    /// directory containing the collection file.
    pub root_dir: PathBuf,
    /// State that should be shared across all renders that use this context.
    /// This is meant to be opaque; just use [Default::default] to initialize.
    pub state: RenderGroupState,
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
        // First, make sure it's a valid recipe. Technically it's possible to
        // return a cached response for a recipe that's no longer in the
        // collection if it existed historically, but this is most likely a
        // mistake by the user. Return an error eagerly to make it easy to debug
        if self.collection.recipes.get_recipe(recipe_id).is_none() {
            return Err(FunctionError::RecipeUnknown {
                recipe_id: recipe_id.clone(),
            });
        }

        let exchange = match trigger {
            RequestTrigger::Never => self
                .get_latest_cached(recipe_id)
                .await?
                .ok_or(FunctionError::ResponseMissing)?,
            RequestTrigger::NoHistory => {
                // If a exchange is present in history, use that. If not, fetch
                if let Some(exchange) =
                    self.get_latest_cached(recipe_id).await?
                {
                    exchange
                } else {
                    self.send_request(recipe_id).await?
                }
            }
            RequestTrigger::Expire { duration } => {
                match self.get_latest_cached(recipe_id).await? {
                    Some(exchange)
                        if exchange.end_time + duration.inner()
                            >= Utc::now() =>
                    {
                        exchange
                    }
                    _ => self.send_request(recipe_id).await?,
                }
            }
            RequestTrigger::Always => self.send_request(recipe_id).await?,
        };

        Ok(exchange.response)
    }

    /// Get the most recent cached exchange for the given recipe
    async fn get_latest_cached(
        &self,
        recipe_id: &RecipeId,
    ) -> Result<Option<Exchange>, FunctionError> {
        self.http_provider
            .get_latest_request(self.selected_profile.as_ref(), recipe_id)
            .await
            .map_err(FunctionError::Database)
    }

    /// Send a request for the recipe and return the exchange
    async fn send_request(
        &self,
        recipe_id: &RecipeId,
    ) -> Result<Exchange, FunctionError> {
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
    }
}

impl slumber_template::Context for TemplateContext {
    async fn get_field(
        &self,
        field: &Identifier,
    ) -> Result<Stream, RenderError> {
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
        template.render_stream(self).await.map_err(|error| {
            // We *could* just return the error, but wrap it to give additional
            // context
            FunctionError::ProfileNested {
                field: field.clone(),
                error,
            }
            .into()
        })
    }

    fn field_cache(&self) -> &FieldCache {
        &self.state.field_cache
    }

    async fn call(
        &self,
        function_name: &Identifier,
        arguments: Arguments<'_, Self>,
    ) -> Result<Stream, RenderError> {
        match function_name.as_str() {
            "base64" => functions::base64(arguments),
            "boolean" => functions::boolean(arguments),
            "command" => functions::command(arguments).await,
            "concat" => functions::concat(arguments),
            "debug" => functions::debug(arguments),
            "env" => functions::env(arguments),
            "file" => functions::file(arguments),
            "float" => functions::float(arguments),
            "integer" => functions::integer(arguments),
            "json_parse" => functions::json_parse(arguments),
            "jsonpath" => functions::jsonpath(arguments),
            "prompt" => functions::prompt(arguments).await,
            "response" => functions::response(arguments).await,
            "response_header" => functions::response_header(arguments).await,
            "select" => functions::select(arguments).await,
            "sensitive" => functions::sensitive(arguments),
            "string" => functions::string(arguments),
            "trim" => functions::trim(arguments),
            _ => Err(RenderError::FunctionUnknown),
        }
    }
}

/// Initialize template context with an empty collection
#[cfg(any(test, feature = "test"))]
impl slumber_util::Factory for TemplateContext {
    fn factory((): ()) -> Self {
        Self::factory((IndexMap::new(), IndexMap::new()))
    }
}

/// Initialize template context with some profiles and recipes. The first
/// profile will be selected
#[cfg(any(test, feature = "test"))]
impl
    slumber_util::Factory<(
        IndexMap<ProfileId, Profile>,
        IndexMap<RecipeId, Recipe>,
    )> for TemplateContext
{
    fn factory(
        (profiles, recipes): (
            IndexMap<ProfileId, Profile>,
            IndexMap<RecipeId, Recipe>,
        ),
    ) -> Self {
        use crate::{
            database::CollectionDatabase,
            test_util::{TestHttpProvider, TestPrompter},
        };
        use slumber_util::paths::get_repo_root;

        let selected_profile = profiles.first().map(|(id, _)| id.clone());
        Self {
            collection: Collection {
                name: None,
                recipes: recipes.into(),
                profiles,
            }
            .into(),
            selected_profile,
            http_provider: Box::new(TestHttpProvider::new(
                CollectionDatabase::factory(()),
                None,
            )),
            overrides: IndexMap::new(),
            prompter: Box::<TestPrompter>::default(),
            root_dir: get_repo_root().to_owned(),
            show_sensitive: true,
            state: Default::default(),
        }
    }
}

/// State for a render group, which consists of one or more related renders
/// (e.g. all the template renders for a single recipe). This state is stored in
/// the template context.
#[derive(Debug, Default)]
pub struct RenderGroupState {
    /// Cache the result of each profile field, so multiple references to the
    /// same field within a render group don't have to do the work multiple
    /// times. If a field fails to render, the guard holder should drop the
    /// guard without entering a result. This will kill the entire render so
    /// other renderers of that field will be cancelled.
    field_cache: FieldCache,
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
    /// Error decoding a base64 string
    #[error(transparent)]
    Base64Decode(#[from] base64::DecodeError),

    /// Error creating or spawning a subprocess
    #[error(
        "Executing command `{}`", iter::once(program).chain(args).format(" ")
    )]
    CommandInit {
        program: String,
        args: Vec<String>,
        #[source]
        error: io::Error,
    },

    /// Command exited with a non-zero status code
    #[error(
        "Command `{command}` exited with {status}\n{stderr}",
        command = iter::once(program).chain(args).format(" "),
        stderr = String::from_utf8_lossy(stderr),
    )]
    CommandStatus {
        program: String,
        args: Vec<String>,
        status: ExitStatus,
        // Storing stdout+stderr because I like symmetry. It's not easy to
        // print both because we don't know how they're supposed to be
        // interleaved. The error should be in stderr so we'll just print that.
        stdout: Vec<u8>,
        stderr: Vec<u8>,
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

    /// Error parsing JSON data
    #[error("Error parsing JSON")]
    JsonParse(
        #[from]
        #[source]
        serde_json::Error,
    ),

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

    /// An bubbled-up error from rendering a profile field value
    #[error("Rendering profile field `{field}`")]
    ProfileNested {
        field: Identifier,
        #[source]
        error: RenderError,
    },

    /// Never got a reply from the prompt channel. Do *not* store the
    /// `RecvError` here, because it provides useless extra output to the user.
    #[error("No reply from prompt")]
    PromptNoReply,

    /// Recipe for `response()`/`response_header()` is not in the collection
    #[error("Unknown recipe `{recipe_id}`")]
    RecipeUnknown { recipe_id: RecipeId },

    /// Recipe for `response()`/`response_header()` has no history
    #[error("No response available")]
    ResponseMissing,

    /// Specified header did not exist in the response
    #[error("Header `{header}` not in response")]
    ResponseMissingHeader { header: String },

    /// `select()` was given no options to display. There's no way for us to
    /// return a meaningful reply
    #[error("Select has no options")]
    SelectNoOptions,

    /// Never got a reply from the select channel. Do *not* store the
    /// `RecvError` here, because it provides useless extra output to the user.
    #[error("No reply from select")]
    SelectNoReply,

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

impl From<FunctionError> for RenderError {
    fn from(error: FunctionError) -> Self {
        RenderError::other(error)
    }
}
