//! Template rendering tools. This is a wrapper around the template engine from
//! [slumber_template], with context and functions specific to rendering HTTP
//! requests.

mod functions;
#[cfg(test)]
mod tests;
mod util;

#[cfg(any(test, feature = "test"))]
use crate::collection::Recipe;
use crate::{
    collection::{Collection, Profile, ProfileId, RecipeId},
    http::{
        Exchange, RequestSeed, ResponseRecord, StoredRequestError,
        TriggeredRequestError,
    },
    render::{
        functions::RequestTrigger,
        util::{FieldCache, FieldCacheOutcome},
    },
};
use async_trait::async_trait;
use chrono::Utc;
use derive_more::{Deref, From, derive::Display};
use indexmap::IndexMap;
use itertools::Itertools;
use serde::Deserialize;
use slumber_template::{
    Arguments, Identifier, LazyValue, RenderError, Template, Value,
};
use std::{
    fmt::Debug, io, iter, path::PathBuf, process::ExitStatus, sync::Arc,
};
use thiserror::Error;
use tokio::sync::oneshot;
use tracing::error;

/// A little container struct for all the data that the user can access via
/// templating. Unfortunately this has to own all data so templating can be
/// deferred into a task (tokio requires `'static` for spawned tasks). If this
/// becomes a bottleneck, we can `Arc` some stuff.
///
/// One instance of this context applies to all renders in a group (a render
/// group is all the renders for a single request). [SingleRenderContext] is a
/// wrapper for each individual render. Use [Self::streaming] to get the wrapper
/// for a render.
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
    /// Additional profile key=value overrides passed directly from the user.
    /// These will be applied to both the root and triggered requests, which is
    /// why they are part of the context instead of
    /// [BuildOptions](super::http::BuildOptions).
    pub overrides: IndexMap<String, Template>,
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
    ///  Wrap this context for a single render, with streaming optionally
    /// enabled
    pub fn streaming(&self, can_stream: bool) -> SingleRenderContext<'_> {
        SingleRenderContext {
            context: self,
            can_stream,
        }
    }

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
            .map_err(FunctionError::StoredRequest)
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

/// A wrapper for [TemplateContext] that provides the
/// [slumber_template::Context] trait. While [TemplateContext] is intended to be
/// used for multiple renders within a render group, this is meant for an
/// individual render. As such, it captures settings that can vary across
/// different renders in the same group.
#[derive(Debug, Deref)]
pub struct SingleRenderContext<'a> {
    #[deref]
    context: &'a TemplateContext,
    /// Is streaming supported for this component? Enabled for request bodies,
    /// disabled for everything else.
    can_stream: bool,
}

impl slumber_template::Context for SingleRenderContext<'_> {
    fn can_stream(&self) -> bool {
        self.can_stream
    }

    async fn get_field(
        &self,
        field: &Identifier,
    ) -> Result<LazyValue, RenderError> {
        // Check the field cache to see if this value is already being computed
        // somewhere else. If it is, we'll block on that and re-use the result.
        // If not, we get a guard back, meaning we're responsible for the
        // computation. At the end, we'll write back to the guard so everyone
        // else can copy our homework.
        let guard = match self
            .context
            .state
            .field_cache
            .get_or_init(field.clone())
            .await
        {
            FieldCacheOutcome::Hit(value) => return Ok(value.into()),
            FieldCacheOutcome::Miss(guard) => guard,
        };

        // We're responsible for the computation. Grab the field's value
        let template = self
            // Check overrides first
            .context
            .overrides
            .get(field.as_str())
            .or_else(|| {
                // Check the current profile
                let profile = self.context.current_profile()?;
                profile.data.get(field.as_str())
            })
            .ok_or_else(|| FunctionError::UnknownField {
                field: field.to_string(),
            })?;

        // Render the nested template
        let output = template.render(self).await;

        // If the output is a value, we can cache it. If it's a stream, it can't
        // be cloned so it can't be cached. In practice there's probably no
        // reason to include the same stream field twice in a single body, but
        // if that happens we'll have to compute it twice. This saves us a lot
        // of annoying machinery though.
        if output.has_stream() {
            // If the nested template rendered to a single chunk, we can unpack
            // it out of its chunk list. If it had multiple chunks, we need to
            // keep all of them to provide both a correct preview and the final
            // stream
            Ok(output.unpack())
        } else {
            let value = output.try_collect_value().await.map_err(
                // We *could* just return the error, but wrap it to give
                // additional context
                |error| {
                    RenderError::from(FunctionError::ProfileNested {
                        field: field.clone(),
                        error,
                    })
                },
            )?;
            guard.set(value.clone());
            Ok(LazyValue::Value(value))
        }
    }

    async fn call(
        &self,
        function_name: &Identifier,
        arguments: Arguments<'_, Self>,
    ) -> Result<LazyValue, RenderError> {
        match function_name.as_str() {
            "base64" => functions::base64(arguments),
            "boolean" => functions::boolean(arguments),
            "command" => functions::command(arguments),
            "concat" => functions::concat(arguments),
            "debug" => functions::debug(arguments),
            "env" => functions::env(arguments),
            "file" => functions::file(arguments),
            "float" => functions::float(arguments),
            "index" => functions::index(arguments),
            "integer" => functions::integer(arguments),
            "join" => functions::join(arguments),
            "jq" => functions::jq(arguments),
            "json_parse" => functions::json_parse(arguments),
            "jsonpath" => functions::jsonpath(arguments),
            "lower" => functions::lower(arguments),
            "prompt" => functions::prompt(arguments).await,
            "replace" => functions::replace(arguments),
            "response" => functions::response(arguments).await,
            "response_header" => functions::response_header(arguments).await,
            "select" => functions::select(arguments).await,
            "sensitive" => functions::sensitive(arguments),
            "slice" => functions::slice(arguments),
            "split" => functions::split(arguments),
            "string" => functions::string(arguments),
            "trim" => functions::trim(arguments),
            "upper" => functions::upper(arguments),
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
        use slumber_util::test_data_dir;

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
            root_dir: test_data_dir(),
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
#[async_trait(?Send)] // Native async fn isn't dyn-compatible
pub trait HttpProvider: Debug {
    /// Get the most recent request for a particular profile+recipe
    async fn get_latest_request(
        &self,
        profile_id: Option<&ProfileId>,
        recipe_id: &RecipeId,
    ) -> Result<Option<Exchange>, StoredRequestError>;

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
/// requests. This happens if a template has multiple prompt values or if
/// multiple templates with prompts are being rendered simultaneously. The
/// implementor is responsible for queueing prompts to show to the user one at a
/// time.
pub trait Prompter: Debug {
    /// Ask the user a question, and use the given channel to return a response.
    /// To indicate "no response", simply drop the returner.
    ///
    /// If an error occurs while prompting the user, just drop the returner.
    /// The implementor is responsible for logging the error as appropriate.
    fn prompt(&self, prompt: Prompt);
}

/// Data defining a prompt which should be presented to the user
#[derive(Debug)]
pub enum Prompt {
    /// Ask the user for text input
    Text {
        /// Tell the user what we're asking for
        message: String,
        /// Value used to pre-populate the text box
        default: Option<String>,
        /// Should the value the user is typing be masked? E.g. password input
        sensitive: bool,
        /// How the prompter will pass the answer back
        channel: ReplyChannel<String>,
    },
    /// Ask the user to pick a value from a list
    Select {
        /// Tell the user what we're asking for
        message: String,
        /// List of choices the user can pick from. This will never be empty.
        options: Vec<SelectOption>,
        /// How the prompter will pass the answer back. The returned value is
        /// the `value` field from the selected [SelectOption]
        channel: ReplyChannel<Value>,
    },
}

/// An entry in a `select()` list
#[derive(Clone, Debug, Display, Deserialize)]
#[display("{label}")]
pub struct SelectOption {
    /// Label to display to the user for this option
    pub label: String,
    /// Underlying value to return if this option is selected. This will be the
    /// same as the label if the input was a single string.
    pub value: Value,
}

/// Channel used to return a reply to a one-time request. This is its own type
/// so we can provide wrapping functionality
#[derive(Debug, From)]
pub struct ReplyChannel<T>(oneshot::Sender<T>);

impl<T> ReplyChannel<T> {
    /// Return the value that the user gave
    pub fn reply(self, reply: T) {
        // This error *shouldn't* ever happen, because the templating task
        // stays open until it gets a reply
        if self.0.send(reply).is_err() {
            error!("Reply listener dropped");
        }
    }
}

/// An error that can occur within a template function
#[derive(Debug, Error)]
pub enum FunctionError {
    /// Error decoding a base64 string
    #[error(transparent)]
    Base64Decode(#[from] base64::DecodeError),

    /// Error creating, spawning, or executing a subprocess
    #[error(
        "Executing command `{}`", iter::once(program).chain(arguments).format(" ")
    )]
    CommandInit {
        program: String,
        arguments: Vec<String>,
        #[source]
        error: io::Error,
    },

    /// Command exited with a non-zero status code
    #[error(
        "Command `{command}` exited with {status}",
        command = iter::once(program).chain(arguments).format(" "),
    )]
    CommandStatus {
        program: String,
        arguments: Vec<String>,
        status: ExitStatus,
    },

    /// User passed an empty command arrary
    #[error("Command must have at least one element")]
    CommandEmpty,

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

    /// Error executing a jq error. [jaq_core::Error] doesn't impl `Error` or
    /// `Send` so we just stringify it
    #[error("{0}")]
    Jq(String),

    /// Error parsing JSON data
    #[error("Error parsing JSON")]
    JsonParse(
        #[from]
        #[source]
        serde_json::Error,
    ),

    /// jq/JSONPath query returned no results when it should have
    #[error("No results from JSON query `{query}`")]
    JsonQueryNoResults { query: String },

    /// jq/JSONPath query returned 2+ results when we expected 1
    #[error(
        "Expected exactly one result from JSON query `{query}`, \
        but got {actual_count}"
    )]
    JsonQueryTooMany { query: String, actual_count: usize },

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

    /// Invalid regular expression given
    ///
    /// [regex::Error] is pretty descriptive so we don't need extra context.
    #[error(transparent)]
    Regex(#[from] regex::Error),

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

    /// An error occurred while pulling a previous request for a recipe. This
    /// error is generated by our code so we don't need any extra context.
    #[error(transparent)]
    StoredRequest(StoredRequestError),

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
