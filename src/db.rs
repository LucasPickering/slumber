//! The database is responsible for persisting data, including requests and
//! responses.

use crate::{
    collection::{CollectionId, RequestRecipeId},
    http::{Request, RequestId, RequestRecord, Response},
    util::{Directory, ResultExt},
};
use anyhow::Context;
use rusqlite::{
    types::{FromSql, FromSqlError, FromSqlResult, ToSqlOutput, ValueRef},
    Connection, OptionalExtension, Row, ToSql,
};
use rusqlite_migration::{Migrations, M};
use std::{ops::Deref, path::PathBuf, sync::Arc};
use tokio::sync::Mutex;
use tracing::debug;
use uuid::Uuid;

/// A SQLite database for persisting data. Generally speaking, any error that
/// occurs *after* opening the DB connection should be an internal bug, but
/// should be shown to the user whenever possible. All operations are async
/// to enable concurrent accesses to yield. Do not call block on thisfrom the
/// draw phase; instead, cache the results in UI state for as long as they're
/// needed.
///
/// This uses an `Arc` internally, so it's safe and cheap to clone.
///
/// Note: Despite all the operations being async, the actual database isn't
/// async. Each operation will asynchronously wait for the connection mutex,
/// then block while performing the operation. This is just a shortcut, if it
/// becomes a bottleneck we can change that.
#[derive(Clone, Debug)]
pub struct Database {
    /// Data is stored in a sqlite DB. Mutex is needed for multi-threaded
    /// access. This is a bottleneck but the access rate should be so low that
    /// it doesn't matter.
    connection: Arc<Mutex<Connection>>,
}

impl Database {
    /// Load the database. This will perform first-time setup, so this should
    /// only be called at the main session entrypoint.
    pub fn load(collection_id: &CollectionId) -> anyhow::Result<Self> {
        let mut connection = Connection::open(Self::path(collection_id)?)?;
        // Use WAL for concurrency
        connection.pragma_update(None, "journal_mode", "WAL")?;
        Self::migrate(&mut connection)?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
        })
    }

    /// Path to the database file. This will create the directory if it doesn't
    /// exist
    fn path(collection_id: &CollectionId) -> anyhow::Result<PathBuf> {
        Ok(Directory::data(collection_id)
            .create()?
            .join("state.sqlite"))
    }

    /// Apply database migrations
    fn migrate(connection: &mut Connection) -> anyhow::Result<()> {
        let migrations = Migrations::new(vec![M::up(
            // The request state kind is a bit hard to map to tabular data.
            // Everything that we need to query on (HTTP status code,
            // end_time, etc.) is in its own column. The
            // request/repsonse and response will be serialized into
            // msgpack bytes
            "CREATE TABLE requests (
                    id              UUID PRIMARY KEY,
                    recipe_id       TEXT,
                    start_time      TEXT,
                    end_time        TEXT,
                    request         BLOB,
                    response        BLOB,
                    status_code     INTEGER
                )",
        )
        .down("DROP TABLE requests")]);
        migrations.to_latest(connection)?;
        Ok(())
    }

    /// Get the most recent request+response for a recipe, or `None` if there
    /// has never been one received.
    pub async fn get_last_request(
        &self,
        recipe_id: &RequestRecipeId,
    ) -> anyhow::Result<Option<RequestRecord>> {
        self.connection
            .lock()
            .await
            .query_row(
                "SELECT * FROM requests WHERE recipe_id = ?1
                ORDER BY start_time DESC LIMIT 1",
                [recipe_id],
                |row| row.try_into(),
            )
            .optional()
            .context("Error fetching request ID from database")
            .traced()
    }

    /// Add a new request to history. The HTTP engine is responsible for
    /// inserting its own requests. Only requests that received a valid HTTP
    /// response should be stored. In-flight requests, invalid requests, and
    /// requests that failed to complete (e.g. because of a network error)
    /// should not (and cannot) be stored.
    pub async fn insert_request(
        &self,
        record: &RequestRecord,
    ) -> anyhow::Result<()> {
        debug!(
            id = %record.id(),
            url = %record.request.url,
            "Adding request record to database",
        );
        self.connection
            .lock()
            .await
            .execute(
                "INSERT INTO
                requests (
                    id,
                    recipe_id,
                    start_time,
                    end_time,
                    request,
                    response,
                    status_code
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                (
                    record.id(),
                    &record.request.recipe_id,
                    &record.start_time,
                    &record.end_time,
                    &record.request,
                    &record.response,
                    record.response.status.as_u16(),
                ),
            )
            .context("Error saving request to database")
            .traced()?;
        Ok(())
    }
}

/// Test-only helpers
#[cfg(test)]
impl Database {
    /// Create an in-memory DB, only for testing
    pub fn testing() -> Self {
        let mut connection = Connection::open_in_memory().unwrap();
        Self::migrate(&mut connection).unwrap();
        Self {
            connection: Arc::new(Mutex::new(connection)),
        }
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

/// Convert from `SELECT * FROM requests` to `RequestRecord`
impl<'a, 'b> TryFrom<&'a Row<'b>> for RequestRecord {
    type Error = rusqlite::Error;

    fn try_from(row: &Row<'a>) -> Result<Self, Self::Error> {
        Ok(Self {
            id: row.get("id")?,
            start_time: row.get("start_time")?,
            end_time: row.get("end_time")?,
            request: row.get("request")?,
            response: row.get("response")?,
        })
    }
}
