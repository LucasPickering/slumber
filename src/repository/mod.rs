//! The repository persists all requests and responses. It consists of a SQLite
//! database to persist all request history and an in-memory layer to provide
//! caching and other ephemeral data (e.g. prettified content).

mod database;

use crate::{
    config::RequestRecipeId,
    http::{Request, RequestId, Response},
    repository::database::RepositoryDatabase,
};
use anyhow::anyhow;
use chrono::{DateTime, Duration, Utc};
use derive_more::Display;
use lru::LruCache;
use std::{num::NonZeroUsize, sync::Arc};
use strum::{EnumDiscriminants, EnumString};
use tokio::sync::RwLock;

/// Number of requests/responses to cache in memory
const CACHE_SIZE: usize = 10;

/// A record of all HTTP history, which is persisted on disk. This is also used
/// to populate chained values. This uses a sqlite DB underneath, which means
/// all operations are internally fallible. Generally speaking, any error that
/// occurs *after* opening the DB connection should be an internal bug, but
/// should be shown to the user whenever possible. All operations are also
/// async because of the database and the locking nature of the interior cache.
/// They generally will be fast though, so it's safe to block on them in the
/// draw phase.
///
/// Invalid requests should *not* be stored in the repository, because they were
/// never launched.
///
/// Requests and responses are cached in memory, to prevent constantly going to
/// disk and deserializing. The cache uses interior mutability to minimize
/// impact on the external API.
///
/// This is freely cloneable.
#[derive(Clone, Debug)]
pub struct Repository {
    /// The persistence layer
    database: RepositoryDatabase,

    /// Cache all request records that have been created/modified/loaded during
    /// this session, so we don't have to go to the DB on every frame.
    ///
    /// Outer `Arc`/`RwLock` is needed to update the cache for read operations.
    /// `Arc` on each record is needed to return cached records without needing
    /// to keep the lock open.
    ///
    /// Inspired by https://matklad.github.io/2022/06/11/caches-in-rust.html
    request_cache: Arc<RwLock<LruCache<RequestId, Arc<RequestRecord>>>>,
}

/// A single request+response in history
#[derive(Debug)]
pub struct RequestRecord {
    /// When was the request registered in history? This should be very close
    /// to when it was sent to the server
    pub start_time: DateTime<Utc>,
    pub request: Request,
    pub response: ResponseState,
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
            // NEW NEW NEW
            request_cache: Arc::new(RwLock::new(LruCache::new(
                NonZeroUsize::new(CACHE_SIZE).unwrap(),
            ))),
        })
    }

    /// Get a request by ID. Requires `&mut self` so the cache can be updated if
    /// necessary. Return an error if the request isn't in history.
    pub async fn get_request(
        &self,
        request_id: RequestId,
    ) -> anyhow::Result<Arc<RequestRecord>> {
        let mut cache = self.request_cache.write().await;
        match cache.get(&request_id) {
            Some(record) => Ok(Arc::clone(record)),
            None => {
                // Miss - get the request from the DB
                let record =
                    Arc::new(self.database.get_request(request_id).await?);
                cache.push(request_id, Arc::clone(&record));
                Ok::<_, anyhow::Error>(record)
            }
        }
    }

    /// Get the most recent request for a recipe, or `None` if there has never
    /// been one sent
    pub async fn get_last(
        &self,
        recipe_id: &RequestRecipeId,
    ) -> anyhow::Result<Option<Arc<RequestRecord>>> {
        // Find the ID we care about, then fetch the record separately since it
        // may be cached
        Ok(match self.database.get_last(recipe_id).await? {
            Some(request_id) => Some(self.get_request(request_id).await?),
            None => None,
        })
    }

    /// Get the most recent *successful* response for a recipe, or `None` if
    /// there is none. The response state of the returned record is guaranteed
    /// to be variant `Success`.
    pub async fn get_last_success(
        &self,
        recipe_id: &RequestRecipeId,
    ) -> anyhow::Result<Option<Arc<RequestRecord>>> {
        // Find the ID we care about, then fetch the record separately since it
        // may be cached
        Ok(match self.database.get_last_success(recipe_id).await? {
            Some(request_id) => Some(self.get_request(request_id).await?),
            None => None,
        })
    }

    /// Add a new request to history. This should be called immediately before
    /// or after the request is sent, so the generated start_time timestamp
    /// is accurate. Returns the generated record.
    ///
    /// The returned record is wrapped in `Arc` so it can co-exist in our local
    /// cache.
    pub async fn add_request(
        &mut self,
        request: Request,
    ) -> anyhow::Result<Arc<RequestRecord>> {
        let record = Arc::new(RequestRecord {
            request,
            start_time: Utc::now(),
            response: ResponseState::Loading,
        });
        self.database.add_request(&record).await?;
        self.cache_record(Arc::clone(&record)).await;
        Ok(record)
    }

    /// Attach a response (or error) to an existing request. Errors will be
    /// converted to a string for serialization.
    pub async fn add_response(
        &mut self,
        request_id: RequestId,
        // The error is stored as a string, so take anything stringifiable
        result: Result<Response, impl ToString>,
    ) -> anyhow::Result<Arc<RequestRecord>> {
        let end_time = Utc::now();
        let response_state = match result {
            Ok(response) => ResponseState::Success { response, end_time },
            Err(err) => ResponseState::Error {
                error: err.to_string(),
                end_time,
            },
        };

        // Update in the DB, which will kick back the updated record. Stick
        // that new guy in the cache
        let updated_record = Arc::new(
            self.database
                .add_response(request_id, &response_state)
                .await?,
        );
        self.cache_record(Arc::clone(&updated_record)).await;

        Ok(updated_record)
    }

    /// Store a request record in the cache.
    async fn cache_record(&mut self, record: Arc<RequestRecord>) {
        let record_id = record.id();
        let mut cache = self.request_cache.write().await;
        cache.push(record_id, record);
    }
}

/// Test-only helpers
#[cfg(test)]
impl Repository {
    /// Create an in-memory repository DB, only for testing
    pub fn testing() -> Self {
        Self {
            database: RepositoryDatabase::testing(),
            request_cache: Arc::new(RwLock::new(LruCache::new(
                NonZeroUsize::new(CACHE_SIZE).unwrap(),
            ))),
        }
    }

    /// Add a request-response pair
    pub async fn add(
        &mut self,
        request: Request,
        response: anyhow::Result<Response>,
    ) {
        let record = self.add_request(request).await.unwrap();
        self.add_response(record.id(), response).await.unwrap();
    }
}

impl RequestRecord {
    /// Get the unique ID for this request
    pub fn id(&self) -> RequestId {
        self.request.id
    }

    /// Unpack the response state as a successful response. If it isn't a
    /// success, return an error.
    pub fn try_response(&self) -> anyhow::Result<&Response> {
        match &self.response {
            ResponseState::Success { response, .. } => Ok(response),
            other => Err(anyhow!("Request is in non-success state {other:?}")),
        }
    }

    /// Get the elapsed time for this request, according to response state:
    /// - Loading - Elapsed time since the request started
    /// - Incomplete - `None`
    /// - Success - Duration from start to loading the entire request
    /// - Error - Duration from start to request failing
    pub fn duration(&self) -> Option<Duration> {
        match &self.response {
            ResponseState::Loading => Some(Utc::now() - self.start_time),
            ResponseState::Incomplete => None,
            ResponseState::Success { end_time, .. }
            | ResponseState::Error { end_time, .. } => {
                Some(*end_time - self.start_time)
            }
        }
    }
}
