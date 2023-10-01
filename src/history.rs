use crate::{
    config::RequestRecipeId,
    http::{Request, RequestId, Response},
    util::ResultExt,
};
use anyhow::{bail, Context};
use chrono::{DateTime, Duration, Utc};
use derive_more::Display;
use lru::LruCache;
use rusqlite::{
    types::{FromSql, FromSqlError, FromSqlResult, ToSqlOutput, ValueRef},
    Connection, OptionalExtension, Row, ToSql,
};
use rusqlite_migration::{Migrations, M};
use std::{
    cell::RefCell, num::NonZeroUsize, ops::Deref, path::PathBuf, rc::Rc,
};
use strum::{EnumDiscriminants, EnumString};
use tracing::debug;
use uuid::Uuid;

/// Number of requests/responses to cache in memory
const CACHE_SIZE: usize = 10;

/// A record of all HTTP history, which is persisted on disk. This is also used
/// to populate chained values. This uses a sqlite DB underneath, which means
/// all operations are internally fallible. Generally speaking, any error that
/// occurs *after* opening the DB connection should be an internal bug. The
/// error should be shown to the user whenever possible.
///
/// Invalid requests should *not* be stored in history, because they were never
/// launched.
///
/// Requests and responses are cached in memory, to prevent constantly going to
/// disk and deserializing. The cache uses interior mutability to minimize
/// impact on the external API.
#[derive(Debug)]
pub struct RequestHistory {
    /// History is stored in a sqlite DB, for ease of access and insertion. It
    /// feels a little overkill, but to get the random lookup and persistence
    /// that we need, the equivalent Rust struct would start to look a lot
    /// like a database anyway.
    db_connection: Connection,
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

impl RequestHistory {
    /// Path to the history database file
    fn path() -> PathBuf {
        // TODO use path in home dir
        PathBuf::from("./history.sqlite")
    }

    /// Load the history database. This will perform first-time setup, so this
    /// should only be called at the main session entrypoint.
    pub fn load() -> anyhow::Result<Self> {
        let db_connection = Connection::open(Self::path())?;
        // Use WAL for concurrency
        db_connection.pragma_update(None, "journal_mode", "WAL")?;

        let mut history = Self {
            db_connection,
            request_cache: RefCell::new(LruCache::new(
                NonZeroUsize::new(CACHE_SIZE).unwrap(),
            )),
        };

        history.setup()?;
        Ok(history)
    }

    /// First-time setup, should be called once per session
    fn setup(&mut self) -> anyhow::Result<()> {
        let migrations = Migrations::new(vec![M::up(
            // The response state kind is a bit hard to map to tabular data.
            // Everything that we need to query on (success/error kind, HTTP
            // status code, end_time, etc.) is in its own column. The response
            // itself will be serialized into text
            "CREATE TABLE requests (
                id              UUID PRIMARY KEY,
                recipe_id       TEXT,
                start_time      TEXT,
                end_time        TEXT NULLABLE,
                request         BLOB,
                response_kind   TEXT,
                response        BLOB NULLABLE,
                status_code     INTEGER NULLABLE
            )",
        )]);
        migrations.to_latest(&mut self.db_connection)?;

        // Anything that was pending when we exited is lost now, so convert
        // those to incomplete
        self.db_connection.execute(
            "UPDATE requests SET response_kind = ?1 WHERE response_kind = ?2",
            (ResponseStateKind::Incomplete, ResponseStateKind::Loading),
        )?;
        Ok(())
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
        debug!(
            id = %request.id,
            url = %request.url,
            "Adding request to history",
        );
        let record = Rc::new(RequestRecord {
            request,
            start_time: Utc::now(),
            response: RefCell::new(ResponseState::Loading),
        });
        self.db_connection
            .execute(
                "INSERT INTO
                requests (id, recipe_id, start_time, request, response_kind)
                VALUES (?1, ?2, ?3, ?4, ?5)",
                (
                    record.id(),
                    &record.request.recipe_id,
                    &record.start_time,
                    &record.request,
                    ResponseStateKind::Loading,
                ),
            )
            .context("Error saving request in history")?;

        Ok(self.cache_record(record))
    }

    /// Attach a response (or error) to an existing request. Errors will be
    /// converted to a string for serialization
    pub fn add_response(
        &mut self,
        request_id: RequestId,
        // The error is stored as a string, so take anything stringifiable
        result: Result<Response, impl ToString>,
    ) -> anyhow::Result<()> {
        debug!(
            %request_id,
            outcome = match result {
                Ok(_) => "OK",
                Err(_) => "Error",
            },
            "Adding response to history"
        );

        let end_time = Utc::now();
        let response_state = match result {
            Ok(response) => ResponseState::Success { response, end_time },
            Err(err) => ResponseState::Error {
                error: err.to_string(),
                end_time,
            },
        };

        // Update the DB first
        let (content, status_code): (&dyn ToSql, Option<u16>) =
            match &response_state {
                ResponseState::Success { response, .. } => {
                    (response, Some(response.status.as_u16()))
                }
                ResponseState::Error { error, .. } => (error, None),
                // We just created the state, so we know it can't hit this
                _ => unreachable!("Response state must be success or error"),
            };
        let updated_rows = self
            .db_connection
            .execute(
                "UPDATE requests SET response_kind = ?1, response = ?2,
                end_time = ?3, status_code = ?4
                WHERE id = ?5",
                (
                    ResponseStateKind::from(&response_state),
                    content,
                    end_time,
                    status_code,
                    request_id,
                ),
            )
            .context("Error saving response in history")?;

        // Safety check, make sure the ID matched
        if updated_rows != 1 {
            bail!(
                "Expected to update 1 row when adding response, \
                but updated {updated_rows} instead"
            )
        }

        // Update the cache with the response
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
            let record: RequestRecord = self
                .db_connection
                .query_row(
                    "SELECT * FROM requests WHERE id = ?1",
                    [request_id],
                    |row| row.try_into(),
                )
                .context("Error fetching request from history")
                .traced()?;
            debug!(%request_id, "Loaded request from history");
            Ok::<_, anyhow::Error>(Rc::new(record))
        })?;
        Ok(Rc::clone(record))
    }

    /// Get the most recent request for a recipe, or `None` if there has never
    /// been one sent
    pub fn get_last(
        &self,
        recipe_id: &RequestRecipeId,
    ) -> anyhow::Result<Option<Rc<RequestRecord>>> {
        // First, find the ID we care about. The fetch the record separately,
        // since it may be cached
        let request_id_opt: Option<RequestId> = self
            .db_connection
            .query_row(
                "SELECT id FROM requests WHERE recipe_id = ?1
                ORDER BY start_time DESC LIMIT 1",
                [recipe_id],
                |row| row.get(0),
            )
            .optional()
            .context("Error fetching request ID from history")
            .traced()?;

        Ok(match request_id_opt {
            Some(request_id) => Some(self.get_request(request_id)?),
            None => None,
        })
    }

    /// Get the most recent *successful* response for a recipe, or `None` if
    /// there is none.
    pub fn get_last_success(
        &self,
        recipe_id: &RequestRecipeId,
    ) -> anyhow::Result<Option<Response>> {
        // Right now this doesn't use the cache because that makes it hard to
        // return just a Response
        let record_opt = self
            .db_connection
            .query_row(
                "SELECT * FROM requests
                WHERE recipe_id = ?1 AND response_kind = ?2
                ORDER BY start_time DESC LIMIT 1",
                (recipe_id, ResponseStateKind::Success),
                |row| row.get("response"),
            )
            .optional()
            .context("Error fetching request from history")
            .traced()?;
        debug!(%recipe_id, "Loaded request from history");
        Ok(record_opt)
    }

    /// Store a request record in the cache. Return a reference to the cached
    /// record
    fn cache_record(&self, record: Rc<RequestRecord>) -> Rc<RequestRecord> {
        let record_id = record.id();
        let mut cache = self.request_cache.borrow_mut();
        cache.push(record_id, record);
        // Grab the thing we just inserted
        let record = cache.get(&record_id).unwrap();
        Rc::clone(record)
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
impl RequestHistory {
    /// Create an in-memory history DB, only for testing
    pub fn testing() -> Self {
        let db_connection = Connection::open_in_memory().unwrap();
        let mut history = Self {
            db_connection,
            request_cache: RefCell::new(LruCache::new(
                NonZeroUsize::new(1).unwrap(),
            )),
        };
        history.setup().unwrap();
        history
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

/// Convert from `SELECT * FROM requests`
impl<'a, 'b> TryFrom<&'a Row<'b>> for RequestRecord {
    type Error = rusqlite::Error;

    fn try_from(row: &Row<'a>) -> Result<Self, Self::Error> {
        // Extract the response based on the response_kind column
        let response = match row.get::<_, ResponseStateKind>("response_kind")? {
            ResponseStateKind::Loading => ResponseState::Loading,
            ResponseStateKind::Incomplete => ResponseState::Incomplete,
            ResponseStateKind::Success => ResponseState::Success {
                response: row.get("response")?,
                end_time: row.get("end_time")?,
            },
            ResponseStateKind::Error => ResponseState::Error {
                error: row.get("response")?,
                end_time: row.get("end_time")?,
            },
        };

        Ok(Self {
            request: row.get("request")?,
            start_time: row.get("start_time")?,
            response: RefCell::new(response),
        })
    }
}

impl ToSql for RequestId {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        self.deref().to_sql()
    }
}

impl FromSql for RequestId {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        Ok(Uuid::column_result(value)?.into())
    }
}

impl ToSql for RequestRecipeId {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        self.deref().to_sql()
    }
}

impl FromSql for RequestRecipeId {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        Ok(String::column_result(value)?.into())
    }
}

impl ToSql for ResponseStateKind {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::Owned(self.to_string().into()))
    }
}

impl FromSql for ResponseStateKind {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        String::column_result(value)?
            .parse()
            .map_err(|err| FromSqlError::Other(Box::new(err)))
    }
}

/// Macro to convert a serializable type to/from SQL via MessagePack
macro_rules! serial_sql {
    ($t:ty) => {
        impl ToSql for $t {
            fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
                let bytes = rmp_serde::to_vec(self).map_err(|err| {
                    rusqlite::Error::ToSqlConversionFailure(Box::new(err))
                })?;
                Ok(ToSqlOutput::Owned(bytes.into()))
            }
        }

        impl FromSql for $t {
            fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
                let bytes = value.as_blob()?;
                rmp_serde::from_slice(bytes)
                    .map_err(|err| FromSqlError::Other(Box::new(err)))
            }
        }
    };
}

serial_sql!(Request);
serial_sql!(Response);
