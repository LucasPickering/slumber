//! The repository persists all requests and responses. It consists of a SQLite
//! database to persist all request history and an in-memory layer to provide
//! caching and other ephemeral data (e.g. prettified content).

mod database;

use crate::{
    config::RequestRecipeId,
    http::{Request, RequestId, Response},
    repository::database::RepositoryDatabase,
};
use chrono::{DateTime, Duration, Utc};
use derive_more::Display;
use lru::LruCache;
use std::{cell::RefCell, num::NonZeroUsize, ops::Deref, rc::Rc};
use strum::{EnumDiscriminants, EnumString};

/// Number of requests/responses to cache in memory
const CACHE_SIZE: usize = 10;

/// A record of all HTTP history, which is persisted on disk. This is also used
/// to populate chained values. This uses a sqlite DB underneath, which means
/// all operations are internally fallible. Generally speaking, any error that
/// occurs *after* opening the DB connection should be an internal bug. The
/// error should be shown to the user whenever possible.
///
/// Invalid requests should *not* be stored in the repository, because they were
/// never launched.
///
/// Requests and responses are cached in memory, to prevent constantly going to
/// disk and deserializing. The cache uses interior mutability to minimize
/// impact on the external API.
#[derive(Debug)]
pub struct Repository {
    /// The persistence layer
    database: RepositoryDatabase,

    /// Cache all request records that have been created/modified/loaded during
    /// this session, so we don't have to go to the DB on every frame.
    ///
    /// The `RefCell` allows us to cache retrieved values loaded from the
    /// database without requiring the user to hold `&mut` for a read
    /// operation. The `Rc` allows the caller to retain a reference to the
    /// record without having to keep the `RefCell` open.
    ///
    /// Inspired by https://matklad.github.io/2022/06/11/caches-in-rust.html
    request_cache: RefCell<LruCache<RequestId, Rc<RequestRecord>>>,
}

/// A single request+response in history
#[derive(Debug)]
pub struct RequestRecord {
    /// When was the request registered in history? This should be very close
    /// to when it was sent to the server
    pub start_time: DateTime<Utc>,
    pub request: Request,
    /// This needs interior mutability so the response can be modified after
    /// the request is already cached. We guarantee soundness by only mutating
    /// during the message phase of the TUI.
    response: RefCell<ResponseState>,
}

/// State of an HTTP response, which can be pending or completed. Also generate
/// a discriminant-only enum that will map to the `response_kind` column
#[derive(Debug, EnumDiscriminants)]
#[strum_discriminants(name(ResponseStateKind), derive(Display, EnumString))]
pub enum ResponseState {
    /// Request is in flight, or is *about* to be sent. There's no way to
    /// initiate a request that doesn't immediately launch it, so Loading is
    /// the initial state.
    Loading,
    /// The request never terminated because the program exited while it was
    /// in flight. We have no idea of knowing how long it took, so this is
    /// stored separately from the error state.
    Incomplete,

    // TODO try factoring terminal variants into their own enum
    /// A resolved HTTP response, with all content loaded and ready to be
    /// displayed. This does *not necessarily* have a 2xx/3xx status code, any
    /// received response is considered a "success".
    Success {
        response: Response,
        /// When did we finish receiving the full response?
        end_time: DateTime<Utc>,
    },
    /// Error occurred sending the request or receiving the response. We're
    /// never going to do anything with the error but display it, so just
    /// store it as a string. This makes it easy to display to the user and
    /// serialize/deserialize.
    Error {
        error: String,
        /// When did the error occur?
        end_time: DateTime<Utc>,
    },
}

impl Repository {
    /// Load the repository from the underlying database
    pub fn load() -> anyhow::Result<Self> {
        Ok(Self {
            database: RepositoryDatabase::load()?,
            request_cache: RefCell::new(LruCache::new(
                NonZeroUsize::new(CACHE_SIZE).unwrap(),
            )),
        })
    }

    /// Add a new request to history. This should be called immediately before
    /// or after the request is sent, so the generated start_time timestamp
    /// is accurate. Returns the generated record.
    ///
    /// The returned record is wrapped in `Rc` so it can co-exist in our local
    /// cache.
    pub fn add_request(
        &mut self,
        request: Request,
    ) -> anyhow::Result<Rc<RequestRecord>> {
        let record = Rc::new(RequestRecord {
            request,
            start_time: Utc::now(),
            response: RefCell::new(ResponseState::Loading),
        });
        self.database.add_request(&record)?;
        self.cache_record(Rc::clone(&record));
        Ok(record)
    }

    /// Attach a response (or error) to an existing request. Errors will be
    /// converted to a string for serialization.
    pub fn add_response(
        &mut self,
        request_id: RequestId,
        // The error is stored as a string, so take anything stringifiable
        result: Result<Response, impl ToString>,
    ) -> anyhow::Result<()> {
        let end_time = Utc::now();
        let response_state = match result {
            Ok(response) => ResponseState::Success { response, end_time },
            Err(err) => ResponseState::Error {
                error: err.to_string(),
                end_time,
            },
        };

        // Update the DB (because it only needs a reference), then the cache
        self.database.add_response(request_id, &response_state)?;
        self.cache_response(request_id, response_state);

        Ok(())
    }

    /// Get a request by ID. Requires `&mut self` so the cache can be updated if
    /// necessary. Return an error if the request isn't in history.
    pub fn get_request(
        &self,
        request_id: RequestId,
    ) -> anyhow::Result<Rc<RequestRecord>> {
        let mut cache = self.request_cache.borrow_mut();
        let record = cache.try_get_or_insert(request_id, || {
            // Miss - get the request from the DB
            let record = Rc::new(self.database.get_request(request_id)?);
            Ok::<_, anyhow::Error>(record)
        })?;
        Ok(Rc::clone(record))
    }

    /// Get the most recent request for a recipe, or `None` if there has never
    /// been one sent
    pub fn get_last(
        &self,
        recipe_id: &RequestRecipeId,
    ) -> anyhow::Result<Option<Rc<RequestRecord>>> {
        // Find the ID we care about, then fetch the record separately since it
        // may be cached
        self.database
            .get_last(recipe_id)?
            .map(|request_id| self.get_request(request_id))
            .transpose()
    }

    /// Get the most recent *successful* response for a recipe, or `None` if
    /// there is none. The response state of the returned record is guaranteed
    /// to be variant `Success`.
    pub fn get_last_success(
        &self,
        recipe_id: &RequestRecipeId,
    ) -> anyhow::Result<Option<Rc<RequestRecord>>> {
        // Find the ID we care about, then fetch the record separately since it
        // may be cached
        self.database
            .get_last_success(recipe_id)?
            .map(|request_id| self.get_request(request_id))
            .transpose()
    }

    /// Store a request record in the cache.
    fn cache_record(&self, record: Rc<RequestRecord>) {
        let record_id = record.id();
        let mut cache = self.request_cache.borrow_mut();
        cache.push(record_id, record);
    }

    /// Link a response to an existing request in the cache. If the request
    /// isn't in the cache, do nothing.
    fn cache_response(
        &mut self,
        request_id: RequestId,
        response_state: ResponseState,
    ) {
        // Use peek_mut so we don't update the LRU. A response coming in
        // doesn't necessarily mean the user still cares about it.
        if let Some(record) = self.request_cache.get_mut().peek_mut(&request_id)
        {
            *record.response.borrow_mut() = response_state;
        }
    }
}

/// Test-only helpers
#[cfg(test)]
impl Repository {
    /// Create an in-memory repository DB, only for testing
    pub fn testing() -> Self {
        Self {
            database: RepositoryDatabase::testing(),
            request_cache: RefCell::new(LruCache::new(
                NonZeroUsize::new(CACHE_SIZE).unwrap(),
            )),
        }
    }

    /// Add a request-response pair
    pub fn add(
        &mut self,
        request: Request,
        response: anyhow::Result<Response>,
    ) {
        let record = self.add_request(request).unwrap();
        self.add_response(record.id(), response).unwrap();
    }
}

impl RequestRecord {
    /// Get the unique ID for this request
    pub fn id(&self) -> RequestId {
        self.request.id
    }

    /// Access the response state for this request. Return `impl Deref` to mask
    /// the implementation details of interior mutability here.
    pub fn response(&self) -> impl Deref<Target = ResponseState> + '_ {
        self.response.borrow()
    }

    /// Get the elapsed time for this request, according to response state:
    /// - Loading - Elapsed time since the request started
    /// - Incomplete - `None`
    /// - Success - Duration from start to loading the entire request
    /// - Error - Duration from start to request failing
    pub fn duration(&self) -> Option<Duration> {
        match self.response().deref() {
            ResponseState::Loading => Some(Utc::now() - self.start_time),
            ResponseState::Incomplete => None,
            ResponseState::Success { end_time, .. }
            | ResponseState::Error { end_time, .. } => {
                Some(*end_time - self.start_time)
            }
        }
    }
}
