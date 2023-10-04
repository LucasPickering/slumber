//! The repository persists all requests and responses. It consists of a SQLite
//! database to persist all request history and an in-memory layer to provide
//! caching and other ephemeral data (e.g. prettified content).

mod database;
mod parse;

pub use parse::ParsedBody;

use crate::{
    config::RequestRecipeId,
    http::{Request, RequestId, Response},
    repository::database::RepositoryDatabase,
};
use anyhow::anyhow;
use chrono::{DateTime, Duration, Utc};
use derive_more::Display;
use futures::Future;
use lru::LruCache;
use std::{hash::Hash, num::NonZeroUsize, ops::Deref, sync::Arc};
use strum::{EnumDiscriminants, EnumString};
use tokio::{sync::RwLock, task};
use tracing::trace;

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
    request_cache: Cache<RequestId, RequestRecord>,

    /// Cache every response body that we've attempted to parse
    parsed_body_cache: Cache<RequestId, ParsedBody>,
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
            request_cache: Cache::new(),
            parsed_body_cache: Cache::new(),
        })
    }

    /// Get a request by ID. Requires `&mut self` so the cache can be updated if
    /// necessary. Return an error if the request isn't in history.
    pub async fn get_request(
        &self,
        request_id: RequestId,
    ) -> anyhow::Result<Arc<RequestRecord>> {
        self.request_cache
            .try_get_or_insert(
                request_id,
                self.database.get_request(request_id),
            )
            .await
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

    /// Get the parsed form of a record's response body. The given record must
    /// be in the Success state. If the parsed body is already cached that will
    /// be returned, otherwise it will be parsed and cached for the next time.
    /// The parsing will be done *in a separate task*, so this will not block
    /// while parsing.
    pub async fn get_parsed_body(
        &self,
        record: Arc<RequestRecord>,
    ) -> anyhow::Result<Arc<ParsedBody>> {
        // Errors in parsing will *not* be cached. Typically a user won't
        // retry a failed operation very much, and if they do they'd probably
        // be happy to know it actually tried it again
        self.parsed_body_cache
            .try_get_or_insert(record.id(), async {
                task::spawn_blocking(move || {
                    trace!(
                        request_id = %record.id(),
                        "Parsing response body"
                    );
                    ParsedBody::parse(record.try_response()?)
                })
                // Unpack the potential JoinError
                .await?
            })
            .await
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
            request_cache: Cache::new(),
            parsed_body_cache: Cache::new(),
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

/// A threadsafe LRU cache
#[derive(Debug)]
struct Cache<K: Eq + Hash + PartialEq, V>(Arc<RwLock<LruCache<K, Arc<V>>>>);

impl<K: Eq + Hash + PartialEq, V> Cache<K, V> {
    /// All caches use the same size, for no reason beyond simplicity
    const CACHE_SIZE: usize = 10;

    /// Build a new cache with a fixed size
    fn new() -> Self {
        Self(Arc::new(RwLock::new(LruCache::new(
            NonZeroUsize::new(Self::CACHE_SIZE).unwrap(),
        ))))
    }

    /// Get a value from the cache, or insert a new value generated from the
    /// given function if it isn't already in the cache. The getter future
    /// is expected to be fallible, and will only be resolved if the lookup
    /// fails. Similar to [LruCache::try_get_or_insert] but it supports
    /// async.
    async fn try_get_or_insert(
        &self,
        key: K,
        future: impl Future<Output = anyhow::Result<V>>,
    ) -> anyhow::Result<Arc<V>> {
        match self.write().await.get(&key) {
            Some(value) => Ok(Arc::clone(value)),
            None => {
                // Miss - use the function to get the value
                let value = Arc::new(future.await?);
                // Reopen the lock so we don't hold it across an async bound
                self.write().await.push(key, Arc::clone(&value));
                Ok::<_, anyhow::Error>(value)
            }
        }
    }
}

impl<K: Eq + Hash + PartialEq, V> Deref for Cache<K, V> {
    type Target = RwLock<LruCache<K, Arc<V>>>;

    fn deref(&self) -> &Self::Target {
        self.0.deref()
    }
}

/// Derive macro applies a Clone bound to K and V which is no good
impl<K: Eq + Hash + PartialEq, V> Clone for Cache<K, V> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<K: Eq + Hash + PartialEq, V> Default for Cache<K, V> {
    fn default() -> Self {
        Self::new()
    }
}
