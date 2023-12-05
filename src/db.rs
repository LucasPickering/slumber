//! The database is responsible for persisting data, including requests and
//! responses.

use crate::{
    collection::{CollectionId, RequestRecipeId},
    http::{RequestId, RequestRecord},
    util::{Directory, ResultExt},
};
use anyhow::Context;
use rusqlite::{
    types::{FromSql, FromSqlError, FromSqlResult, ToSqlOutput, ValueRef},
    Connection, OptionalExtension, Row, ToSql,
};
use rusqlite_migration::{Migrations, M};
use serde::{de::DeserializeOwned, Serialize};
use std::{
    fmt::{Debug, Display},
    ops::Deref,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tracing::debug;
use uuid::Uuid;

/// A SQLite database for persisting data. Generally speaking, any error that
/// occurs *after* opening the DB connection should be an internal bug, but
/// should be shown to the user whenever possible. All operations are blocking,
/// to enable calling from the view code. Do not call on every frame though,
/// cache results in UI state for as long as they're needed.
///
/// This uses an `Arc` internally, so it's safe and cheap to clone.
#[derive(Clone, Debug)]
pub struct Database {
    /// Data is stored in a sqlite DB. Mutex is needed for multi-threaded
    /// access. This is a bottleneck but the access rate should be so low that
    /// it doesn't matter. If it does become a bottleneck, we could spawn
    /// one connection per thread, but the code would be a bit more
    /// complicated.
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
        let migrations = Migrations::new(vec![
            M::up(
                // The request state kind is a bit hard to map to tabular data.
                // Everything that we need to query on (HTTP status code,
                // end_time, etc.) is in its own column. Therequest/response
                // will be serialized into msgpack bytes
                "CREATE TABLE requests (
                    id              UUID PRIMARY KEY NOT NULL,
                    recipe_id       TEXT NOT NULL,
                    start_time      TEXT NOT NULL,
                    end_time        TEXT NOT NULL,
                    request         BLOB NOT NULL,
                    response        BLOB NOT NULL,
                    status_code     INTEGER NOT NULL
                )",
            )
            .down("DROP TABLE requests"),
            M::up(
                // Values will be serialized as msgpack
                "CREATE TABLE ui_state (
                key         TEXT PRIMARY KEY NOT NULL,
                value       BLOB NOT NULL
            )",
            )
            .down("DROP TABLE ui_state"),
        ]);
        migrations.to_latest(connection)?;
        Ok(())
    }

    /// Get a reference to the DB connection. Panics if the lock is poisoned
    fn connection(&self) -> impl '_ + Deref<Target = Connection> {
        self.connection.lock().expect("Connection lock poisoned")
    }

    /// Get the most recent request+response for a recipe, or `None` if there
    /// has never been one received.
    pub fn get_last_request(
        &self,
        recipe_id: &RequestRecipeId,
    ) -> anyhow::Result<Option<RequestRecord>> {
        self.connection()
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
    pub fn insert_request(&self, record: &RequestRecord) -> anyhow::Result<()> {
        debug!(
            id = %record.id(),
            url = %record.request.url,
            "Adding request record to database",
        );
        self.connection()
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
                    &Bytes(&record.request),
                    &Bytes(&record.response),
                    record.response.status.as_u16(),
                ),
            )
            .context("Error saving request to database")
            .traced()?;
        Ok(())
    }

    /// Get the value of a UI state field
    pub fn get_ui<K, V>(&self, key: K) -> anyhow::Result<Option<V>>
    where
        K: Display,
        V: Debug + DeserializeOwned,
    {
        let value = self
            .connection()
            .query_row(
                "SELECT value FROM ui_state WHERE key = ?1",
                (key.to_string(),),
                |row| {
                    let value: Bytes<V> = row.get(0)?;
                    Ok(value.0)
                },
            )
            .optional()
            .context("Error fetching UI state from database")
            .traced()?;
        debug!(%key, ?value, "Fetched UI state");
        Ok(value)
    }

    /// Set the value of a UI state field
    pub fn set_ui<K, V>(&self, key: K, value: V) -> anyhow::Result<()>
    where
        K: Display,
        V: Debug + Serialize,
    {
        debug!(%key, ?value, "Setting UI state");
        self.connection()
            .execute(
                // Upsert!
                "INSERT INTO ui_state VALUES (?1, ?2)
                ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                (key.to_string(), Bytes(value)),
            )
            .context("Error saving UI state to database")
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

/// A wrapper to serialize/deserialize a value as msgpack for DB storage
struct Bytes<T>(T);

impl<T: Serialize> ToSql for Bytes<T> {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        let bytes = rmp_serde::to_vec(&self.0).map_err(|err| {
            rusqlite::Error::ToSqlConversionFailure(Box::new(err))
        })?;
        Ok(ToSqlOutput::Owned(bytes.into()))
    }
}

impl<T: DeserializeOwned> FromSql for Bytes<T> {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let bytes = value.as_blob()?;
        let value: T = rmp_serde::from_slice(bytes)
            .map_err(|err| FromSqlError::Other(Box::new(err)))?;
        Ok(Self(value))
    }
}

/// Convert from `SELECT * FROM requests` to `RequestRecord`
impl<'a, 'b> TryFrom<&'a Row<'b>> for RequestRecord {
    type Error = rusqlite::Error;

    fn try_from(row: &Row<'a>) -> Result<Self, Self::Error> {
        Ok(Self {
            id: row.get("id")?,
            start_time: row.get("start_time")?,
            end_time: row.get("end_time")?,
            // Deserialize from bytes
            request: row.get::<_, Bytes<_>>("request")?.0,
            response: row.get::<_, Bytes<_>>("response")?.0,
        })
    }
}
