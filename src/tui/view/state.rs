//! State types for the view.

pub mod fixed_select;
pub mod persistence;
pub mod request_store;
pub mod select;

use crate::{
    collection::{ProfileId, RecipeId},
    http::{
        Exchange, ExchangeSummary, RequestBuildError, RequestError, RequestId,
        RequestRecord,
    },
};
use bytesize::ByteSize;
use chrono::{DateTime, Duration, Utc};
use derive_more::Deref;
use reqwest::StatusCode;
use std::{
    cell::{Ref, RefCell},
    sync::Arc,
};

/// An internally mutable cell for UI state. Certain state needs to be updated
/// during the draw phase, typically because it's derived from parent data
/// passed via props. This is safe to use in the render phase, because rendering
/// is entirely synchronous.
///
/// In addition to storing the state value, this stores a state key as well. The
/// key is used to determine when to update the state. The key should be
/// something cheaply comparable. If the value itself is cheaply comparable,
/// you can just use that as the key.
#[derive(Debug)]
pub struct StateCell<K, V> {
    state: RefCell<Option<(K, V)>>,
}

impl<K, V> StateCell<K, V> {
    /// Get the current state value, or a new value if the state is stale. State
    /// will be stale if it is uninitialized OR the key has changed. In either
    /// case, `init` will be called to create a new value.
    pub fn get_or_update(&self, key: K, init: impl FnOnce() -> V) -> Ref<'_, V>
    where
        K: PartialEq,
    {
        let mut state = self.state.borrow_mut();
        match state.deref() {
            Some(state) if state.0 == key => {}
            _ => {
                // (Re)create the state
                *state = Some((key, init()));
            }
        }
        drop(state);

        // Unwrap is safe because we just stored a value
        // It'd be nice to return an `impl Deref` here instead to prevent
        // leaking implementation details, but I was struggling with the
        // lifetimes on that
        Ref::map(self.state.borrow(), |state| &state.as_ref().unwrap().1)
    }

    /// Get a reference to the state value. This can panic, if the state value
    /// is already borrowed elsewhere. Returns `None` iff the state cell is
    /// uninitialized.
    pub fn get(&self) -> Option<Ref<'_, V>> {
        Ref::filter_map(self.state.borrow(), |state| {
            state.as_ref().map(|(_, v)| v)
        })
        .ok()
    }

    /// Get a mutable reference to the state value. This will never panic
    /// because `&mut self` guarantees exclusive access. Returns `None` iff
    /// the state cell is uninitialized.
    pub fn get_mut(&mut self) -> Option<&mut V> {
        self.state.get_mut().as_mut().map(|state| &mut state.1)
    }
}

/// Derive impl applies unnecessary bound on the generic parameter
impl<K, V> Default for StateCell<K, V> {
    fn default() -> Self {
        Self {
            state: RefCell::new(None),
        }
    }
}

/// State of an HTTP response, which can be in various states of
/// completion/failure. Each request *recipe* should have one request state
/// stored in the view at a time.
#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq))]
pub enum RequestState {
    /// The request is being built. Typically this is very fast, but can be
    /// slow if a chain source takes a while.
    Building {
        id: RequestId,
        start_time: DateTime<Utc>,
        profile_id: Option<ProfileId>,
        recipe_id: RecipeId,
    },

    /// Something went wrong during the build :(
    BuildError { error: RequestBuildError },

    /// Request is in flight, or is *about* to be sent. There's no way to
    /// initiate a request that doesn't immediately launch it, so Loading is
    /// the initial state.
    Loading {
        /// This needs an Arc so the success/failure state can maintain a
        /// pointer to the request as well
        request: Arc<RequestRecord>,
        start_time: DateTime<Utc>,
    },

    /// A resolved HTTP response, with all content loaded and ready to be
    /// displayed. This does *not necessarily* have a 2xx/3xx status code, any
    /// received response is considered a "success".
    Response { exchange: Exchange },

    /// Error occurred sending the request or receiving the response.
    RequestError { error: RequestError },
}

/// Metadata derived from a request. The request can be in progress, completed,
/// or failed.
#[derive(Debug)]
pub struct RequestMetadata {
    /// When was the request launched?
    pub start_time: DateTime<Utc>,
    /// Elapsed time for the active request. If pending, this is a running
    /// total. Otherwise end time - start time.
    pub duration: Duration,
}

/// Metadata derived from a response. This is only available for requests that
/// have completed successfully.
#[derive(Debug)]
pub struct ResponseMetadata {
    pub status: StatusCode,
    /// Size of the response *body*
    pub size: ByteSize,
}

impl RequestState {
    /// Unique ID for this request, which will be retained throughout its life
    /// cycle
    pub fn id(&self) -> RequestId {
        match self {
            Self::Building { id, .. } => *id,
            Self::BuildError { error, .. } => error.id,
            Self::Loading { request, .. } => request.id,
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
            Self::RequestError { error } => &error.request.recipe_id,
            Self::Response { exchange, .. } => &exchange.request.recipe_id,
        }
    }

    /// Get metadata about a request. Return `None` if the request hasn't been
    /// successfully built (yet)
    pub fn request_metadata(&self) -> Option<RequestMetadata> {
        match self {
            Self::Building { .. } | Self::BuildError { .. } => None,
            Self::Loading { start_time, .. } => Some(RequestMetadata {
                start_time: *start_time,
                duration: Utc::now() - start_time,
            }),
            Self::Response { exchange, .. } => Some(RequestMetadata {
                start_time: exchange.start_time,
                duration: exchange.duration(),
            }),
            Self::RequestError { error } => Some(RequestMetadata {
                start_time: error.start_time,
                duration: error.end_time - error.start_time,
            }),
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

    /// Create a loading state with the current timestamp. This will generally
    /// be slightly off from when the request was actually launched, but it
    /// shouldn't matter. See [crate::http::RequestTicket::send] for why it
    /// can't report a start time back to us.
    pub fn loading(request: Arc<RequestRecord>) -> Self {
        Self::Loading {
            request,
            start_time: Utc::now(),
        }
    }

    /// Create a request state from a completed response. This is **expensive**,
    /// don't call it unless you need the value.
    pub fn response(exchange: Exchange) -> Self {
        // Pre-parse the body so the view doesn't have to do it. We're in the
        // main thread still here though so large bodies may take a while. Maybe
        // we want to punt this into a separate task?
        exchange.response.parse_body();
        Self::Response { exchange }
    }
}

/// A simplified version of [RequestState], which only stores metadata. This is
/// useful when you want to show a list of requests and don't need the entire
/// request/response data for each one.
#[derive(Debug)]
pub enum RequestStateSummary {
    Building {
        id: RequestId,
        start_time: DateTime<Utc>,
    },
    BuildError {
        id: RequestId,
        time: DateTime<Utc>,
    },
    Loading {
        id: RequestId,
        start_time: DateTime<Utc>,
    },
    Response(ExchangeSummary),
    RequestError {
        id: RequestId,
        time: DateTime<Utc>,
    },
}

impl RequestStateSummary {
    pub fn id(&self) -> RequestId {
        match self {
            Self::Building { id, .. }
            | Self::BuildError { id, .. }
            | Self::Loading { id, .. }
            | Self::RequestError { id, .. } => *id,
            Self::Response(exchange) => exchange.id,
        }
    }

    /// Get the time of the request state. For in-flight or completed requests,
    /// this is when it *started*.
    pub fn time(&self) -> DateTime<Utc> {
        match self {
            Self::Building {
                start_time: time, ..
            }
            | Self::BuildError { time, .. }
            | Self::Loading {
                start_time: time, ..
            }
            | Self::RequestError { time, .. } => *time,
            Self::Response(exchange) => exchange.start_time,
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
                time: error.time,
            },
            RequestState::Loading {
                request,
                start_time,
                ..
            } => Self::Loading {
                id: request.id,
                start_time: *start_time,
            },
            RequestState::Response { exchange } => {
                Self::Response(exchange.into())
            }
            RequestState::RequestError { error } => Self::RequestError {
                id: error.request.id,
                time: error.start_time,
            },
        }
    }
}

/// A notification is an ephemeral informational message generated by some async
/// action. It doesn't grab focus, but will be useful to the user nonetheless.
/// It should be shown for a short period of time, then disappear on its own.
#[derive(Debug)]
pub struct Notification {
    pub message: String,
    pub timestamp: DateTime<Utc>,
}

impl Notification {
    pub fn new(message: String) -> Self {
        Self {
            message,
            timestamp: Utc::now(),
        }
    }
}
