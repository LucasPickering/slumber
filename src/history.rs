use crate::{
    config::RequestRecipeId,
    http::{Request, Response},
};
use chrono::{DateTime, Duration, Utc};
use derive_more::Deref;
use rusqlite::{
    types::{FromSql, FromSqlError, FromSqlResult, ToSqlOutput, ValueRef},
    Connection, OptionalExtension, Row, ToSql,
};
use serde::{Deserialize, Serialize};
use std::{ops::Deref, path::PathBuf};
use tracing::debug;
use uuid::Uuid;

// TODO make sqlite async (worth? would require draw code to be async too)

/// A record of all HTTP history, which is persisted on disk. This is also used
/// to populate chained values. This uses a sqlite DB underneath, which means
/// all operations are internally fallible. Generally speaking, any error that
/// occurs *after* opening the DB connection should be an internal bug, because
/// any external changes to the file system will not affect the open file
/// handle. Therefore they are panics. This simplifies the external API.
///
/// Invalid requests should *not* be stored in history, because they were never
/// launched.
#[derive(Debug)]
pub struct RequestHistory {
    /// History is stored in a sqlite DB, for ease of access and insertion. It
    /// feels a little overkill, but to get the random lookup and persistence
    /// that we need, the equivalent Rust struct would start to look a lot
    /// like a database anyway.
    db_connection: Connection,
}

/// Unique ID for a single instance of a request recipe
#[derive(Copy, Clone, Debug, Deref)]
pub struct RequestId(Uuid);

/// A single request in history
#[derive(Debug)]
pub struct RequestRecord {
    /// Uniquely identify this record
    pub id: RequestId,
    pub recipe_id: RequestRecipeId,
    /// When was the request sent to the server?
    pub start_time: DateTime<Utc>,
    /// When did the request either finish or fail? Populated iff response is
    /// `Success`/`Error`.
    pub end_time: Option<DateTime<Utc>>,
    pub request: Request,
    pub response: ResponseState,
}

/// State of an HTTP response, which can be pending or completed.
#[derive(Debug, Serialize, Deserialize)]
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
    Success(Response),
    /// Error occurred sending the request or receiving the response. We're
    /// never going to do anything with the error but display it, so just
    /// store it as a string. This makes it easy to display to the user and
    /// serialize/deserialize.
    Error(String),
}

impl RequestHistory {
    /// Path to the history database file
    fn path() -> PathBuf {
        // TODO use path in home dir
        PathBuf::from("./history.sqlite")
    }

    /// Load history from disk. If the file doesn't exist yet, load a default
    /// value. Any other error will be propagated.
    pub fn load() -> anyhow::Result<Self> {
        // TODO apply error context to fn
        let db_connection = Connection::open(Self::path())?;
        db_connection.execute(
            "CREATE TABLE IF NOT EXISTS requests (
                id          UUID PRIMARY KEY,
                recipe_id   TEXT,
                start_time  TEXT,
                end_time    TEXT NULLABLE,
                request     TEXT,
                response    TEXT
            )",
            [],
        )?;
        // TODO mark Loading requests as errored
        Ok(Self { db_connection })
    }

    /// Add a new request to history. This should be called when the request
    /// is sent, so the generated start_time timestamp is accurate. Returns the
    /// generated ID for the request, so it can be linked to the response later.
    pub fn add_request(
        &mut self,
        recipe_id: &RequestRecipeId,
        request: &Request,
    ) -> RequestId {
        let id = RequestId(Uuid::new_v4());
        debug!(?id, ?recipe_id, ?request, "Adding request to history");
        self.db_connection
            .execute(
                "INSERT INTO
                requests (id, recipe_id, start_time, request, response)
                VALUES (?1, ?2, ?3, ?4, ?5)",
                (id, recipe_id, Utc::now(), request, ResponseState::Loading),
            )
            .expect("Error saving request in history");
        id
    }

    /// Attach a response (or error) to an existing request. Errors will be
    /// converted to a string for serialization
    pub fn add_response(
        &self,
        request_id: RequestId,
        result: anyhow::Result<Response>,
    ) {
        let response = match result {
            Ok(response) => ResponseState::Success(response),
            Err(err) => ResponseState::Error(err.to_string()),
        };

        debug!(?request_id, ?response, "Adding response to history");
        let updated_rows = self
            .db_connection
            .execute(
                "UPDATE requests SET response = ?1, end_time = ?2 WHERE id = ?3",
                (response, Utc::now(), request_id),
            )
            .expect("Error saving response in history");

        // Safety check, make sure it ID matched
        if updated_rows != 1 {
            panic!(
                "Expected to update 1 row when adding response, \
                but updated {updated_rows} instead"
            );
        }
    }

    /// Get the most recent request for a recipe, or `None` if there has never
    /// been one sent
    pub fn get_last(
        &self,
        recipe_id: &RequestRecipeId,
    ) -> Option<RequestRecord> {
        self.db_connection
            .query_row(
                "SELECT * FROM requests WHERE recipe_id = ?1
                ORDER BY start_time DESC LIMIT 1",
                [recipe_id],
                |row| row.try_into(),
            )
            .optional()
            .expect("Error fetching response from history")
    }
}

impl RequestRecord {
    /// Get the elapsed time for this request, according to response state:
    /// - Loading - Elapsed time since the request started
    /// - Incomplete - `None`
    /// - Success - Duration from start to loading the entire request
    /// - Error - Duration from start to request failing
    pub fn duration(&self) -> Option<Duration> {
        match self.response {
            ResponseState::Loading => Some(Utc::now() - self.start_time),
            ResponseState::Incomplete => None,
            ResponseState::Success(_) | ResponseState::Error(_) => Some(
                // yuck
                self.end_time.expect("No end_time for complete request")
                    - self.start_time,
            ),
        }
    }
}

/// Convert from `SELECT * FROM requests`
impl<'a, 'b> TryFrom<&'a Row<'b>> for RequestRecord {
    type Error = rusqlite::Error;

    fn try_from(row: &Row<'a>) -> Result<Self, Self::Error> {
        Ok(Self {
            id: row.get("id")?,
            recipe_id: row.get("recipe_id")?,
            request: row.get("request")?,
            start_time: row.get("start_time")?,
            end_time: row.get("end_time")?,
            response: row.get("response")?,
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
        Ok(Self(Uuid::column_result(value)?))
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

/// Macro to convert a serializable type to/from SQL via YAML serialization.
/// This is a bit ugly but it works.
macro_rules! serial_sql {
    ($t:ty) => {
        impl ToSql for $t {
            fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
                Ok(ToSqlOutput::Owned(
                    serde_yaml::to_string(self).unwrap().into(),
                ))
            }
        }

        impl FromSql for $t {
            fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
                let s = value.as_str()?;
                serde_yaml::from_str(s)
                    .map_err(|err| FromSqlError::Other(Box::new(err)))
            }
        }
    };
}

serial_sql!(Request);
serial_sql!(ResponseState);
