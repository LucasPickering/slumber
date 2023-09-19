use crate::{
    config::RequestRecipeId,
    http::{Request, Response},
};
use anyhow::{anyhow, Context};
use chrono::{DateTime, Duration, Utc};
use derive_more::Deref;
use rusqlite::{
    types::{FromSql, FromSqlError, FromSqlResult, ToSqlOutput, ValueRef},
    Connection, OptionalExtension, Row, ToSql,
};
use rusqlite_migration::{Migrations, M};
use std::{ops::Deref, path::PathBuf};
use strum::{Display, EnumDiscriminants, EnumString};
use tracing::{debug, warn};
use uuid::Uuid;

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
    pub request: Request,
    pub response: ResponseState,
}

/// State of an HTTP response, which can be pending or completed. Also generate
/// a discriminant-only enum that will map to the `status` column in the DB
#[derive(Debug, EnumDiscriminants)]
#[strum_discriminants(name(ResponseStatus), derive(Display, EnumString))]
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
        // TODO could we use a custom error type here?
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

    /// Load history from disk. If the file doesn't exist yet, load a default
    /// value. Any other error will be propagated.
    pub fn load() -> anyhow::Result<Self> {
        let mut db_connection = Connection::open(Self::path())?;

        // The response status is a bit hard to map to tabular data. Everything
        // that we need to query on (success/error status, end_time, etc.) is
        // in its own column. The response itself will be serialized into text
        let migrations = Migrations::new(vec![M::up(
            "CREATE TABLE requests (
                id          UUID PRIMARY KEY,
                recipe_id   TEXT,
                start_time  TEXT,
                end_time    TEXT NULLABLE,
                request     TEXT,
                status      TEXT,
                response    TEXT NULLABLE
            )",
        )]);
        migrations.to_latest(&mut db_connection)?;

        // Anything that was pending when we exited is lost now, so convert
        // those to incomplete
        db_connection.execute(
            "UPDATE requests SET status = ?1 WHERE status = ?2",
            (ResponseStatus::Incomplete, ResponseStatus::Loading),
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
    ) -> anyhow::Result<RequestId> {
        let id = RequestId(Uuid::new_v4());
        debug!(?id, ?recipe_id, ?request, "Adding request to history");
        self.db_connection
            .execute(
                "INSERT INTO
                requests (id, recipe_id, start_time, request, status)
                VALUES (?1, ?2, ?3, ?4, ?5)",
                (id, recipe_id, Utc::now(), request, ResponseStatus::Loading),
            )
            .context("Error saving request in history")?;
        Ok(id)
    }

    /// Attach a response (or error) to an existing request. Errors will be
    /// converted to a string for serialization
    pub fn add_response(
        &self,
        request_id: RequestId,
        result: anyhow::Result<Response>,
    ) -> anyhow::Result<()> {
        let (status, response): (ResponseStatus, Box<dyn ToSql>) = match result
        {
            Ok(response) => {
                debug!(
                    ?request_id,
                    ?response,
                    "Adding response success to history"
                );
                (ResponseStatus::Success, Box::new(response))
            }
            Err(err) => {
                warn!(
                    ?request_id,
                    error = err.deref(),
                    "Adding response error to history"
                );
                (ResponseStatus::Error, Box::new(err.to_string()))
            }
        };

        let updated_rows = self
            .db_connection
            .execute(
                "UPDATE requests SET status = ?1, response = ?2, end_time = ?3 WHERE id = ?4",
                (status, response, Utc::now(), request_id),
            )
            .context("Error saving response in history")?;

        // Safety check, make sure it ID matched
        if updated_rows == 1 {
            Ok(())
        } else {
            Err(anyhow!(
                "Expected to update 1 row when adding response, \
                but updated {updated_rows} instead"
            ))
        }
    }

    /// Get the most recent request for a recipe, or `None` if there has never
    /// been one sent
    pub fn get_last(
        &self,
        recipe_id: &RequestRecipeId,
    ) -> anyhow::Result<Option<RequestRecord>> {
        self.db_connection
            .query_row(
                "SELECT * FROM requests WHERE recipe_id = ?1
                ORDER BY start_time DESC LIMIT 1",
                [recipe_id],
                |row| row.try_into(),
            )
            .optional()
            .context("Error fetching response from history")
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
            ResponseState::Success { end_time, .. }
            | ResponseState::Error { end_time, .. } => {
                Some(end_time - self.start_time)
            }
        }
    }
}

/// Convert from `SELECT * FROM requests`
impl<'a, 'b> TryFrom<&'a Row<'b>> for RequestRecord {
    type Error = rusqlite::Error;

    fn try_from(row: &Row<'a>) -> Result<Self, Self::Error> {
        // Extract the response based on the status column
        let response = match row.get::<_, ResponseStatus>("status")? {
            ResponseStatus::Loading => ResponseState::Loading,
            ResponseStatus::Incomplete => ResponseState::Incomplete,
            ResponseStatus::Success => ResponseState::Success {
                response: row.get("response")?,
                end_time: row.get("end_time")?,
            },
            ResponseStatus::Error => ResponseState::Error {
                error: row.get("response")?,
                end_time: row.get("end_time")?,
            },
        };

        Ok(Self {
            id: row.get("id")?,
            recipe_id: row.get("recipe_id")?,
            request: row.get("request")?,
            start_time: row.get("start_time")?,
            response,
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

impl ToSql for ResponseStatus {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::Owned(self.to_string().into()))
    }
}

impl FromSql for ResponseStatus {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        String::column_result(value)?
            .parse()
            .map_err(|err| FromSqlError::Other(Box::new(err)))
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
serial_sql!(Response);
