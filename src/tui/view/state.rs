//! State types for the view.

pub mod fixed_select;
pub mod persistence;
pub mod select;

use crate::http::{
    Request, RequestBuildError, RequestError, RequestId, RequestRecord,
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

    /// Get a reference to the state key. This can panic, if the state key/value
    /// is already borrowed elsewhere. Returns `None` iff the setate cell is
    /// uninitialized.
    pub fn key(&self) -> Option<Ref<'_, K>> {
        Ref::filter_map(self.state.borrow(), |state| {
            state.as_ref().map(|(k, _)| k)
        })
        .ok()
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
    Building { id: RequestId },

    /// Something went wrong during the build :(
    BuildError { error: RequestBuildError },

    /// Request is in flight, or is *about* to be sent. There's no way to
    /// initiate a request that doesn't immediately launch it, so Loading is
    /// the initial state.
    Loading {
        /// This needs an Arc so the success/failure state can maintain a
        /// pointer to the request as well
        request: Arc<Request>,
        start_time: DateTime<Utc>,
    },

    /// A resolved HTTP response, with all content loaded and ready to be
    /// displayed. This does *not necessarily* have a 2xx/3xx status code, any
    /// received response is considered a "success".
    Response { record: RequestRecord },

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
            Self::Building { id } => *id,
            Self::BuildError { error } => error.id,
            Self::Loading { request, .. } => request.id,
            Self::RequestError { error } => error.request.id,
            Self::Response { record, .. } => record.id,
        }
    }

    /// Is the initial stage in a request life cycle?
    pub fn is_initial(&self) -> bool {
        matches!(self, Self::Building { .. })
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
            Self::Response { record, .. } => Some(RequestMetadata {
                start_time: record.start_time,
                duration: record.duration(),
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
        if let RequestState::Response { record } = self {
            Some(ResponseMetadata {
                status: record.response.status,
                size: record.response.body.size(),
            })
        } else {
            None
        }
    }

    /// Initialize a new request in the `Building` state
    pub fn building(id: RequestId) -> Self {
        Self::Building { id }
    }

    /// Create a loading state with the current timestamp. This will generally
    /// be slightly off from when the request was actually launched, but it
    /// shouldn't matter. See [crate::http::HttpEngine::send] for why it can't
    /// report a start time back to us.
    pub fn loading(request: Arc<Request>) -> Self {
        Self::Loading {
            request,
            start_time: Utc::now(),
        }
    }

    /// Create a request state from a completed response. This is **expensive**,
    /// don't call it unless you need the value.
    pub fn response(record: RequestRecord) -> Self {
        // Pre-parse the body so the view doesn't have to do it. We're in the
        // main thread still here though so large bodies may take a while. Maybe
        // we want to punt this into a separate task?
        record.response.parse_body();
        Self::Response { record }
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
