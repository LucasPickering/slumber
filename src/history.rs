use crate::{
    config::RequestRecipeId,
    http::{Request, RequestId, ResponseState},
};
use chrono::Utc;
use rusqlite::{
    types::{FromSql, FromSqlError, FromSqlResult, ToSqlOutput, ValueRef},
    Connection, OptionalExtension, ToSql,
};
use std::{ops::Deref, path::PathBuf};
use tracing::debug;

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
                request     TEXT,
                response    TEXT
            )",
            [],
        )?;
        Ok(Self { db_connection })
    }

    /// Add a new request to history. This should be called when the request
    /// is sent, so the generated start_time timestamp is accurate.
    pub fn add_request(
        &mut self,
        recipe_id: &RequestRecipeId,
        request: &Request,
    ) {
        debug!(?recipe_id, ?request, "Adding request to history");
        self.db_connection
            .execute(
                "INSERT INTO
                requests (id, recipe_id, start_time, request, response)
                VALUES (?1, ?2, ?3, ?4, ?5)",
                (
                    request.id,
                    recipe_id,
                    Utc::now(),
                    request,
                    ResponseState::Loading,
                ),
            )
            .expect("Error saving request in history");
    }

    /// Attach a response (or error) to an existing request. Errors will be
    /// converted to a string for serialization
    pub fn add_response(
        &self,
        request_id: RequestId,
        response: &ResponseState,
    ) {
        debug!(?request_id, ?response, "Adding response to history");
        self.db_connection
            .execute(
                "UPDATE requests SET response = ?1 WHERE id = ?2",
                (response, request_id),
            )
            .expect("Error saving response in history");
    }

    /// Get the most recent response for a recipe (if any)
    pub fn get_last_response(
        &self,
        recipe_id: &RequestRecipeId,
    ) -> Option<ResponseState> {
        self.db_connection
            .query_row(
                "SELECT response FROM requests WHERE recipe_id = ?1
                ORDER BY start_time DESC LIMIT 1",
                [recipe_id],
                |row| row.get(0),
            )
            .optional()
            .expect("Error fetching response from history")
    }
}

impl ToSql for RequestRecipeId {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        self.deref().to_sql()
    }
}

impl ToSql for RequestId {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        self.deref().to_sql()
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
