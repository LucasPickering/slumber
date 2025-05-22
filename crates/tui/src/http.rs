//! Types for managing HTTP state in the TUI

#[cfg(test)]
mod tests;

use crate::{
    context::TuiContext,
    message::{Message, MessageSender},
};
use anyhow::Context;
use async_trait::async_trait;
use chrono::{DateTime, TimeDelta, Utc};
use itertools::Itertools;
use reqwest::StatusCode;
use slumber_core::{
    collection::{ProfileId, RecipeId},
    database::{CollectionDatabase, ProfileFilter},
    http::{
        Exchange, ExchangeSummary, RequestBuildError, RequestError, RequestId,
        RequestRecord, RequestSeed,
    },
    template::{HttpProvider, TemplateContext, TriggeredRequestError},
};
use std::{
    collections::{HashMap, hash_map::Entry},
    fmt::Debug,
    sync::Arc,
};
use strum::EnumDiscriminants;
use tokio::{sync::oneshot, task::AbortHandle};
use tracing::warn;

/// Simple in-memory "database" for request state. This serves a few purposes:
///
/// - Save all incomplete requests (in-progress or failed) from the current app
///   session. These do *not* get persisted in the database
/// - Cache historical requests from the database. If we're accessing them
///   repeatedly, we don't want to keep going back to the DB.
/// - Provide a simple unified interface over both the in-memory cache and the
///   persistent DB, so callers can simply ask for requests and we only go to
///   the DB when necessary.
///
/// These operations are generally fallible only when the underlying DB
/// operation fails.
#[derive(Debug)]
pub struct RequestStore {
    database: CollectionDatabase,
    requests: HashMap<RequestId, RequestState>,
}

impl RequestStore {
    pub fn new(database: CollectionDatabase) -> Self {
        Self {
            database,
            requests: Default::default(),
        }
    }

    /// Are any requests in flight?
    pub fn has_active_requests(&self) -> bool {
        self.requests
            .values()
            .any(|state| matches!(state, RequestState::Loading { .. }))
    }

    /// Get request state by ID
    pub fn get(&self, id: RequestId) -> Option<&RequestState> {
        self.requests.get(&id)
    }

    /// Insert a new request. This will construct a [RequestState::Building]
    pub fn start(
        &mut self,
        id: RequestId,
        profile_id: Option<ProfileId>,
        recipe_id: RecipeId,
        abort_handle: Option<AbortHandle>,
    ) {
        let state = RequestState::Building {
            id,
            start_time: Utc::now(),
            profile_id,
            recipe_id,
            abort_handle,
        };
        self.requests.insert(id, state);
    }

    /// Mark a request as loading. Return the updated state.
    pub fn loading(&mut self, request: Arc<RequestRecord>) -> &RequestState {
        self.replace(request.id, |state| {
            // Requests should go building->loading, but it's possible it got
            // cancelled right before this was called
            if let RequestState::Building { abort_handle, .. } = state {
                RequestState::Loading {
                    request,
                    // Reset timer
                    start_time: Utc::now(),
                    abort_handle,
                }
            } else {
                // Can't create loading state since we don't have a join handle
                warn!(
                    request = ?state,
                    "Cannot mark request as loading: not in building state",
                );
                state
            }
        })
    }

    /// Mark a request as failed because of a build error. Return the updated
    /// state.
    pub fn build_error(
        &mut self,
        error: Arc<RequestBuildError>,
    ) -> &RequestState {
        // Use replace just to help catch bugs
        self.replace(error.id, |state| {
            // This indicates a bug or race condition (e.g. build cancelled as
            // it finished). Error should always take precedence
            if !matches!(state, RequestState::Building { .. }) {
                warn!(
                    request = ?state,
                    "Unexpected prior state for request build error",
                );
            }

            RequestState::BuildError { error }
        })
    }

    /// Mark a request as successful, i.e. we received a response. Return the
    /// updated state. Caller is responsible for persisting the exchange in the
    /// DB.
    pub fn response(&mut self, exchange: Exchange) -> &RequestState {
        let response_state = RequestState::response(exchange);
        // Use replace just to help catch bugs
        self.replace(response_state.id(), |state| {
            // This indicates a bug or race condition (e.g. request cancelled as
            // it finished). Success should always take precedence
            if !matches!(state, RequestState::Loading { .. }) {
                warn!(
                    request = ?state,
                    "Unexpected prior state for request response",
                );
            }

            response_state
        })
    }

    /// Mark a request as failed because of an HTTP error. Return the updated
    /// state.
    pub fn request_error(&mut self, error: Arc<RequestError>) -> &RequestState {
        // Use replace just to help catch bugs
        self.replace(error.request.id, |state| {
            // This indicates a bug or race condition (e.g. request cancelled as
            // it failed). Error should always take precedence
            if !matches!(state, RequestState::Loading { .. }) {
                warn!(
                    request = ?state,
                    "Unexpected prior state for request error",
                );
            }

            RequestState::RequestError { error }
        })
    }

    /// Cancel a request that is either building or loading. If it's in any
    /// other state, it will be left alone. Return the updated state.
    pub fn cancel(&mut self, id: RequestId) -> &RequestState {
        let end_time = Utc::now();
        self.replace(id, |state| match state {
            RequestState::Building {
                id,
                start_time,
                profile_id,
                recipe_id,
                abort_handle: Some(abort_handle),
            } => {
                abort_handle.abort();
                RequestState::Cancelled {
                    id,
                    recipe_id,
                    profile_id,
                    start_time,
                    end_time,
                }
            }
            RequestState::Loading {
                request,
                start_time,
                abort_handle: Some(abort_handle),
            } => {
                abort_handle.abort();
                RequestState::Cancelled {
                    id,
                    recipe_id: request.recipe_id.clone(),
                    profile_id: request.profile_id.clone(),
                    start_time,
                    end_time,
                }
            }
            state => {
                // If the request failed/finished while the cancel event was
                // queued, don't do anything
                warn!(request = ?state, "Cannot cancel request");
                state
            }
        })
    }

    /// Load a request from the database by ID. If already present in the store,
    /// do *not* update it. Only go to the DB if it's missing. Return the loaded
    /// request. Return `None` only if the ID is not present in the store *or*
    /// the DB.
    pub fn load(
        &mut self,
        id: RequestId,
    ) -> anyhow::Result<Option<&RequestState>> {
        let request = match self.requests.entry(id) {
            Entry::Occupied(entry) => Some(entry.into_mut()),
            Entry::Vacant(entry) => self
                .database
                .get_request(id)?
                .map(|exchange| entry.insert(RequestState::response(exchange))),
        };
        Ok(request.map(|r| &*r))
    }

    /// Get the latest request (by start time) for a specific profile+recipe
    /// combo
    pub fn load_latest(
        &mut self,
        profile_id: Option<&ProfileId>,
        recipe_id: &RecipeId,
    ) -> anyhow::Result<Option<&RequestState>> {
        self.cache_latest_exchange(profile_id, recipe_id)?;

        // Now that the know the most recent completed record is in our local
        // cache, find the most recent record of *any* kind

        Ok(self
            .requests
            .values()
            .filter(|state| {
                profile_id == state.profile_id()
                    && state.recipe_id() == recipe_id
            })
            .max_by_key(|state| state.request_metadata().start_time))
    }

    /// Load the latest (by start time) _completed_ request for a specific
    /// profile+recipe combo
    pub fn load_latest_exchange(
        &mut self,
        profile_id: Option<&ProfileId>,
        recipe_id: &RecipeId,
    ) -> anyhow::Result<Option<&Exchange>> {
        self.cache_latest_exchange(profile_id, recipe_id)?;

        // Now that the know the most recent _persisted_ exchange is cached,
        // find the most recent in the store. This will include unpersisted
        // exchanges as well

        Ok(self
            .requests
            .values()
            .filter_map(|state| match state {
                RequestState::Response { exchange }
                    if profile_id == state.profile_id()
                        && state.recipe_id() == recipe_id =>
                {
                    Some(exchange)
                }
                _ => None,
            })
            .max_by_key(|exchange| exchange.start_time))
    }

    /// Load the most recent matching exchange from the DB and cache it here
    fn cache_latest_exchange(
        &mut self,
        profile_id: Option<&ProfileId>,
        recipe_id: &RecipeId,
    ) -> anyhow::Result<()> {
        let exchange = self
            .database
            .get_latest_request(profile_id.into(), recipe_id)?;
        if let Some(exchange) = exchange {
            // Cache this record if it isn't already
            self.requests
                .entry(exchange.id)
                .or_insert(RequestState::response(exchange));
        }
        Ok(())
    }

    /// Load all historical requests for a recipe+profile, then return the
    /// *entire* set of requests, including in-progress ones. Returned requests
    /// are just summaries, not the full request. This is intended for list
    /// views, so we don't need to load the entire request/response for each
    /// one. Results are sorted by request *start* time, descending.
    pub fn load_summaries<'a>(
        &'a self,
        profile_id: Option<&'a ProfileId>,
        recipe_id: &'a RecipeId,
    ) -> anyhow::Result<impl 'a + Iterator<Item = RequestStateSummary>> {
        // Load summaries from the DB. We do *not* want to insert these into the
        // store, because they don't include request/response data
        let loaded = self
            .database
            .get_recipe_requests(profile_id.into(), recipe_id)?;

        // Find what we have in memory already
        let iter = self
            .requests
            .values()
            .filter(move |state| {
                state.profile_id() == profile_id
                    && state.recipe_id() == recipe_id
            })
            .map(RequestStateSummary::from)
            // Add what we loaded from the DB
            .chain(loaded.into_iter().map(RequestStateSummary::Response))
            // Sort descending
            .sorted_by_key(RequestStateSummary::start_time)
            .rev()
            // De-duplicate double-loaded requests
            .unique_by(RequestStateSummary::id);
        Ok(iter)
    }

    /// Is the given request either building or loading, and does it have an
    /// abort handle? Triggered requests (nested within another request's
    /// render) cannot be cancelled independently.
    pub fn can_cancel(&self, id: RequestId) -> bool {
        matches!(
            self.get(id),
            Some(
                RequestState::Building {
                    abort_handle: Some(_),
                    ..
                } | RequestState::Loading {
                    abort_handle: Some(_),
                    ..
                }
            )
        )
    }

    /// Delete all requests for a specific recipe+profile combo. Return the
    /// number of deleted requests
    pub fn delete_recipe_requests(
        &mut self,
        profile_filter: ProfileFilter,
        recipe_id: &RecipeId,
    ) -> anyhow::Result<usize> {
        self.requests.retain(|_, state| {
            // Keep items that _don't_ match
            !(state.recipe_id() == recipe_id
                && profile_filter.matches(state.profile_id()))
        });
        self.database
            .delete_recipe_requests(profile_filter, recipe_id)
    }

    /// Delete a single request from the store _and_ the database
    pub fn delete_request(&mut self, id: RequestId) -> anyhow::Result<()> {
        self.requests.remove(&id);
        self.database.delete_request(id)?;
        Ok(())
    }

    /// Replace a request state in the store with new state, by mapping it
    /// through a function. This assumes the request state is supposed to be in
    /// the state, so it logs a message if it isn't (panics in debug mode). This
    /// should be used for all state updates whether or not you need the
    /// previous state. This will help catch bugs in debug mode.
    fn replace(
        &mut self,
        id: RequestId,
        f: impl FnOnce(RequestState) -> RequestState,
    ) -> &RequestState {
        // Remove the existing value, map it, then reinsert. We need to remove
        // the value first so we can pass ownership to the fn
        if let Some(state) = self.requests.remove(&id) {
            self.requests.insert(id, f(state));
            &self.requests[&id]
        } else {
            // This indicates a logic error somewhere. Ideally we could just log
            // it instead of crashing, but we need to return a value
            panic!("Cannot replace request {id}: not in store");
        }
    }
}

/// An [HttpProvider] that uses the request store (and by extension the
/// database) to access and persist HTTP requests. This defers operations on the
/// request store through the message pipeline, because we can't have direct
/// access to the request store from a template rendering task. We could solve
/// this with `Rc<RefCell>` instead, but that ends up polluting the request
/// store type signatures a lot. This is self-contained with minimal perf impact
#[derive(Debug)]
pub struct TuiHttpProvider {
    messages_tx: MessageSender,
    /// Are we rendering request previews, or the real deal? This controls
    /// whether we'll send triggered requests or not
    preview: bool,
}

impl TuiHttpProvider {
    pub fn new(messages_tx: MessageSender, preview: bool) -> Self {
        Self {
            messages_tx,
            preview,
        }
    }
}

#[async_trait]
impl HttpProvider for TuiHttpProvider {
    async fn get_latest_request(
        &self,
        profile_id: Option<&ProfileId>,
        recipe_id: &RecipeId,
    ) -> anyhow::Result<Option<Exchange>> {
        // Defer the fetch into a message because we can't access the request
        // store from another task
        let (tx, rx) = oneshot::channel();
        self.messages_tx.send(Message::HttpGetLatest {
            profile_id: profile_id.cloned(),
            recipe_id: recipe_id.clone(),
            channel: tx.into(),
        });
        rx.await.context("Error fetching request")
    }

    async fn send_request(
        &self,
        seed: RequestSeed,
        template_context: &TemplateContext,
    ) -> Result<Exchange, TriggeredRequestError> {
        if self.preview {
            // Previews shouldn't have side effects
            Err(TriggeredRequestError::NotAllowed)
        } else {
            // We'll report start updates back to the main loop as we go, so the
            // chained request is visible in the UI. This isn't strictly
            // necessary, but it's easy and keeps the UI in sync with the
            // underlying state
            let request_id = seed.id;
            let profile_id = template_context.selected_profile.clone();
            let recipe_id = seed.recipe_id.clone();

            self.messages_tx.send(Message::HttpBuildingTriggered {
                id: request_id,
                profile_id,
                recipe_id,
            });

            let ticket = TuiContext::get()
                .http_engine
                .build(seed, template_context)
                .await
                .map_err(Arc::new)
                .inspect_err(|error| {
                    // Report error to the TUI
                    self.messages_tx.send(Message::HttpBuildError {
                        error: Arc::clone(error),
                    });
                })?;

            // Build successful, send it out
            self.messages_tx.send(Message::HttpLoading {
                request: Arc::clone(ticket.record()),
            });

            // Clone the exchange so we can persist it in the DB/store and
            // still return it
            let result = ticket.send().await.map_err(Arc::new);
            self.messages_tx.send(Message::HttpComplete(result.clone()));
            result.map_err(TriggeredRequestError::Send)
        }
    }
}

/// State of an HTTP response, which can be in various states of
/// completion/failure. Each request *recipe* should have one request state
/// stored in the view at a time.
#[derive(Debug, EnumDiscriminants)]
#[strum_discriminants(name(RequestStateType))]
pub enum RequestState {
    /// The request is being built. Typically this is very fast, but can be
    /// slow if a chain source takes a while.
    Building {
        id: RequestId,
        start_time: DateTime<Utc>,
        profile_id: Option<ProfileId>,
        recipe_id: RecipeId,
        /// A handle to abort the task running the request. Used to cancel the
        /// request. `None` for triggered requests, because they don't run at
        /// the root of a task and therefore can't be aborted independently.
        abort_handle: Option<AbortHandle>,
    },

    /// Something went wrong during the build :(
    BuildError { error: Arc<RequestBuildError> },

    /// Request is in flight, or is *about* to be sent. There's no way to
    /// initiate a request that doesn't immediately launch it, so Loading is
    /// the initial state.
    Loading {
        /// This needs an Arc so the success/failure state can maintain a
        /// pointer to the request as well
        request: Arc<RequestRecord>,
        start_time: DateTime<Utc>,
        /// A handle to abort the task running the request. Used to cancel the
        /// request. `None` for triggered requests, because they don't run at
        /// the root of a task and therefore can't be aborted independently.
        abort_handle: Option<AbortHandle>,
    },

    /// User cancelled the request mid-flight. We don't store the request here,
    /// just the metadata, because we could've cancelled during build OR load.
    /// We could split this into two different states to handle that, but not
    /// worth.
    Cancelled {
        id: RequestId,
        recipe_id: RecipeId,
        profile_id: Option<ProfileId>,
        start_time: DateTime<Utc>,
        end_time: DateTime<Utc>,
    },

    /// A resolved HTTP response, with all content loaded and ready to be
    /// displayed. This does *not necessarily* have a 2xx/3xx status code, any
    /// received response is considered a "success".
    Response { exchange: Exchange },

    /// Error occurred sending the request or receiving the response.
    RequestError {
        /// This needs an `Arc` so it can be shared with the template engine in
        /// the case of triggered chained requests
        error: Arc<RequestError>,
    },
}

impl RequestState {
    /// Unique ID for this request, which will be retained throughout its life
    /// cycle
    pub fn id(&self) -> RequestId {
        match self {
            Self::Building { id, .. } => *id,
            Self::BuildError { error, .. } => error.id,
            Self::Loading { request, .. } => request.id,
            Self::Cancelled { id, .. } => *id,
            Self::RequestError { error } => error.request.id,
            Self::Response { exchange, .. } => exchange.id,
        }
    }

    /// The profile that the request was rendered from
    pub fn profile_id(&self) -> Option<&ProfileId> {
        match self {
            Self::Building { profile_id, .. } => profile_id.as_ref(),
            Self::BuildError { error } => error.profile_id.as_ref(),
            Self::Loading { request, .. } => request.profile_id.as_ref(),
            Self::Cancelled { profile_id, .. } => profile_id.as_ref(),
            Self::RequestError { error } => error.request.profile_id.as_ref(),
            Self::Response { exchange, .. } => {
                exchange.request.profile_id.as_ref()
            }
        }
    }

    /// The recipe that the request was rendered from
    pub fn recipe_id(&self) -> &RecipeId {
        match self {
            Self::Building { recipe_id, .. } => recipe_id,
            Self::BuildError { error } => &error.recipe_id,
            Self::Loading { request, .. } => &request.recipe_id,
            Self::Cancelled { recipe_id, .. } => recipe_id,
            Self::RequestError { error } => &error.request.recipe_id,
            Self::Response { exchange, .. } => &exchange.request.recipe_id,
        }
    }

    /// Get metadata about a request. Return `None` if the request hasn't been
    /// successfully built (yet)
    pub fn request_metadata(&self) -> RequestMetadata {
        match self {
            // In-progress states
            Self::Building { start_time, .. }
            | Self::Loading { start_time, .. } => RequestMetadata {
                start_time: *start_time,
                end_time: None,
            },

            // Error states
            Self::BuildError { error } => RequestMetadata {
                start_time: error.start_time,
                end_time: Some(error.end_time),
            },
            Self::Cancelled {
                start_time,
                end_time,
                ..
            } => RequestMetadata {
                start_time: *start_time,
                end_time: Some(*end_time),
            },
            Self::RequestError { error } => RequestMetadata {
                start_time: error.start_time,
                end_time: Some(error.end_time),
            },

            // Completed
            Self::Response { exchange, .. } => RequestMetadata {
                start_time: exchange.start_time,
                end_time: Some(exchange.end_time),
            },
        }
    }

    /// Get metadata about the request. Return `None` if the response hasn't
    /// been received, or the request failed.
    pub fn response_metadata(&self) -> Option<ResponseMetadata> {
        if let RequestState::Response { exchange } = self {
            Some(ResponseMetadata {
                status: exchange.response.status,
                size: exchange.response.body.size(),
            })
        } else {
            None
        }
    }

    /// Create a request state from a completed response
    fn response(exchange: Exchange) -> Self {
        Self::Response { exchange }
    }
}

#[cfg(test)]
impl PartialEq for RequestState {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::Building {
                    id: l_id,
                    start_time: l_start_time,
                    profile_id: l_profile_id,
                    recipe_id: l_recipe_id,
                    abort_handle: _,
                },
                Self::Building {
                    id: r_id,
                    start_time: r_start_time,
                    profile_id: r_profile_id,
                    recipe_id: r_recipe_id,
                    abort_handle: _,
                },
            ) => {
                l_id == r_id
                    && l_start_time == r_start_time
                    && l_profile_id == r_profile_id
                    && l_recipe_id == r_recipe_id
            }
            (
                Self::BuildError { error: l_error },
                Self::BuildError { error: r_error },
            ) => l_error == r_error,
            (
                Self::Loading {
                    request: l_request,
                    start_time: l_start_time,
                    abort_handle: _,
                },
                Self::Loading {
                    request: r_request,
                    start_time: r_start_time,
                    abort_handle: _,
                },
            ) => l_request == r_request && l_start_time == r_start_time,
            (
                Self::Cancelled {
                    id: l_id,
                    recipe_id: l_recipe_id,
                    profile_id: l_profile_id,
                    start_time: l_start_time,
                    end_time: l_end_time,
                },
                Self::Cancelled {
                    id: r_id,
                    recipe_id: r_recipe_id,
                    profile_id: r_profile_id,
                    start_time: r_start_time,
                    end_time: r_end_time,
                },
            ) => {
                l_id == r_id
                    && l_recipe_id == r_recipe_id
                    && l_profile_id == r_profile_id
                    && l_start_time == r_start_time
                    && l_end_time == r_end_time
            }
            (
                Self::Response {
                    exchange: l_exchange,
                },
                Self::Response {
                    exchange: r_exchange,
                },
            ) => l_exchange == r_exchange,
            (
                Self::RequestError { error: l_error },
                Self::RequestError { error: r_error },
            ) => l_error == r_error,
            _ => false,
        }
    }
}

/// Metadata derived from a request. The request can be in progress, completed,
/// or failed.
#[derive(Debug)]
pub struct RequestMetadata {
    /// When was the request launched?
    pub start_time: DateTime<Utc>,
    /// When did the request end? This could be when the response came back, or
    /// the request failed/was cancelled. `None` if still loading.
    pub end_time: Option<DateTime<Utc>>,
}

impl RequestMetadata {
    /// Elapsed time for this request. If pending, this is a running total.
    /// Otherwise end time - start time.
    pub fn duration(&self) -> TimeDelta {
        let end_time = self.end_time.unwrap_or_else(Utc::now);
        end_time - self.start_time
    }
}

/// Metadata derived from a response. This is only available for requests that
/// have completed successfully.
#[derive(Copy, Clone, Debug)]
pub struct ResponseMetadata {
    pub status: StatusCode,
    /// Size of the response *body*
    pub size: usize,
}

/// A simplified version of [RequestState], which only stores metadata. This is
/// useful when you want to show a list of requests and don't need the entire
/// request/response data for each one.
#[derive(Debug, PartialEq)]
pub enum RequestStateSummary {
    Building {
        id: RequestId,
        start_time: DateTime<Utc>,
    },
    BuildError {
        id: RequestId,
        start_time: DateTime<Utc>,
        end_time: DateTime<Utc>,
    },
    Loading {
        id: RequestId,
        start_time: DateTime<Utc>,
    },
    Cancelled {
        id: RequestId,
        start_time: DateTime<Utc>,
        end_time: DateTime<Utc>,
    },
    Response(ExchangeSummary),
    RequestError {
        id: RequestId,
        start_time: DateTime<Utc>,
        end_time: DateTime<Utc>,
    },
}

impl RequestStateSummary {
    pub fn id(&self) -> RequestId {
        match self {
            Self::Building { id, .. }
            | Self::BuildError { id, .. }
            | Self::Loading { id, .. }
            | Self::Cancelled { id, .. }
            | Self::RequestError { id, .. } => *id,
            Self::Response(exchange) => exchange.id,
        }
    }

    /// Get the start time of the request state. For in-flight or completed
    /// requests, this is when it *started*.
    pub fn start_time(&self) -> DateTime<Utc> {
        match self {
            Self::Building { start_time, .. }
            | Self::BuildError { start_time, .. }
            | Self::Loading { start_time, .. }
            | Self::Cancelled { start_time, .. }
            | Self::RequestError { start_time, .. } => *start_time,
            Self::Response(exchange) => exchange.start_time,
        }
    }

    /// Elapsed time for the active request. If pending, this is a running
    /// total. Otherwise end time - start time.
    pub fn duration(&self) -> TimeDelta {
        // It'd be nice to dedupe this with the calculation used for
        // RequestMetadata, but it's not that easy
        match self {
            // In-progress states
            Self::Building { start_time, .. }
            | Self::Loading { start_time, .. } => Utc::now() - start_time,

            // Error states
            Self::BuildError {
                start_time,
                end_time,
                ..
            }
            | Self::Cancelled {
                start_time,
                end_time,
                ..
            }
            | Self::RequestError {
                start_time,
                end_time,
                ..
            } => *end_time - *start_time,

            // Completed
            Self::Response(exchange) => exchange.end_time - exchange.start_time,
        }
    }
}

impl From<&RequestState> for RequestStateSummary {
    fn from(state: &RequestState) -> Self {
        match state {
            RequestState::Building { id, start_time, .. } => Self::Building {
                id: *id,
                start_time: *start_time,
            },
            RequestState::BuildError { error } => Self::BuildError {
                id: error.id,
                start_time: error.start_time,
                end_time: error.end_time,
            },
            RequestState::Loading {
                request,
                start_time,
                ..
            } => Self::Loading {
                id: request.id,
                start_time: *start_time,
            },
            RequestState::Cancelled {
                id,
                start_time,
                end_time,
                ..
            } => Self::Cancelled {
                id: *id,
                start_time: *start_time,
                end_time: *end_time,
            },
            RequestState::Response { exchange } => {
                Self::Response(exchange.summary())
            }
            RequestState::RequestError { error } => Self::RequestError {
                id: error.request.id,
                start_time: error.start_time,
                end_time: error.end_time,
            },
        }
    }
}
