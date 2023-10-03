use crate::{
    config::RequestRecipeId,
    http::{Request, RequestId, Response},
    repository::{RequestRecord, ResponseState, ResponseStateKind},
    util::ResultExt,
};
use anyhow::{anyhow, bail, Context};
use chrono::{DateTime, Utc};
use rusqlite::{
    types::{FromSql, FromSqlError, FromSqlResult, ToSqlOutput, ValueRef},
    Connection, OptionalExtension, Row, ToSql,
};
use rusqlite_migration::{Migrations, M};
use std::{cell::RefCell, ops::Deref, path::PathBuf};
use tracing::debug;
use uuid::Uuid;

/// The backing database for the request repository. The data store is sqlite3
/// persisted to disk.
#[derive(Debug)]
pub struct RepositoryDatabase {
    /// History is stored in a sqlite DB
    connection: Connection,
}

impl RepositoryDatabase {
    /// Path to the history database file
    fn path() -> PathBuf {
        // TODO use path in home dir
        PathBuf::from("./history.sqlite")
    }

    /// Load the repository database. This will perform first-time setup, so
    /// this should only be called at the main session entrypoint.
    pub fn load() -> anyhow::Result<Self> {
        let connection = Connection::open(Self::path())?;
        // Use WAL for concurrency
        connection.pragma_update(None, "journal_mode", "WAL")?;
        let mut repository = Self { connection };
        repository.setup()?;

        Ok(repository)
    }

    /// Apply first-time setup
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
        migrations.to_latest(&mut self.connection)?;

        // Anything that was pending when we exited is lost now, so convert
        // those to incomplete
        self.connection.execute(
            "UPDATE requests SET response_kind = ?1 WHERE response_kind = ?2",
            (ResponseStateKind::Incomplete, ResponseStateKind::Loading),
        )?;
        Ok(())
    }

    /// Add a new request to history. This should be called immediately before
    /// or after the request is sent, so the generated start_time timestamp
    /// is accurate.
    pub fn add_request(&self, record: &RequestRecord) -> anyhow::Result<()> {
        debug!(
            id = %record.id(),
            url = %record.request.url,
            "Adding request to database",
        );
        self.connection
            .execute(
                "INSERT INTO
                requests (id, recipe_id, start_time, request, response_kind)
                VALUES (?1, ?2, ?3, ?4, ?5)",
                (
                    record.id(),
                    &record.request.recipe_id,
                    &record.start_time,
                    &record.request,
                    ResponseStateKind::from(record.response.borrow().deref()),
                ),
            )
            .context("Error saving request to database")?;
        Ok(())
    }

    /// Attach a response (or error) to an existing request. Errors will be
    /// converted to a string for serialization. The given response state must
    /// be either the `Success` or `Error` variant.
    pub fn add_response(
        &self,
        request_id: RequestId,
        response_state: &ResponseState,
    ) -> anyhow::Result<()> {
        // This unpack is pretty ugly... we could clean it up by factoring the
        // Success+Error variants into their own type
        let (description, content, status_code, end_time): (
            &str,
            &dyn ToSql,
            Option<u16>,
            &DateTime<Utc>,
        ) = match &response_state {
            ResponseState::Success { response, end_time } => {
                ("OK", response, Some(response.status.as_u16()), end_time)
            }
            ResponseState::Error { error, end_time } => {
                ("Error", error, None, end_time)
            }
            // This indicates a bug in the parent
            _ => bail!("Response state must be success or error"),
        };

        debug!(
            %request_id,
            outcome = description,
            "Adding response to database"
        );

        let updated_rows = self
            .connection
            .execute(
                "UPDATE requests SET response_kind = ?1, response = ?2,
                end_time = ?3, status_code = ?4
                WHERE id = ?5",
                (
                    ResponseStateKind::from(response_state),
                    content,
                    end_time,
                    status_code,
                    request_id,
                ),
            )
            .context("Error saving response to database")?;

        // Safety check, make sure the ID matched
        if updated_rows == 1 {
            Ok(())
        } else {
            Err(anyhow!(
                "Expected to update 1 row when adding response, \
                but updated {updated_rows} instead"
            ))
        }
    }

    /// Get a request by ID. Return an error if the request isn't in history or
    /// the lookup fails.
    pub fn get_request(
        &self,
        request_id: RequestId,
    ) -> anyhow::Result<RequestRecord> {
        let record: RequestRecord = self
            .connection
            .query_row(
                "SELECT * FROM requests WHERE id = ?1",
                [request_id],
                |row| row.try_into(),
            )
            .context("Error fetching request from database")
            .traced()?;
        debug!(%request_id, "Loaded request from database");
        Ok(record)
    }

    /// Get the ID most recent request for a recipe, or `None` if there has
    /// never been one sent
    pub fn get_last(
        &self,
        recipe_id: &RequestRecipeId,
    ) -> anyhow::Result<Option<RequestId>> {
        self.connection
            .query_row(
                "SELECT id FROM requests WHERE recipe_id = ?1
                ORDER BY start_time DESC LIMIT 1",
                [recipe_id],
                |row| row.get(0),
            )
            .optional()
            .context("Error fetching request ID from database")
            .traced()
    }

    /// Get the ID of the most recent *successful* response for a recipe, or
    /// `None` if there is none
    pub fn get_last_success(
        &self,
        recipe_id: &RequestRecipeId,
    ) -> anyhow::Result<Option<RequestId>> {
        self.connection
            .query_row(
                "SELECT id FROM requests
                WHERE recipe_id = ?1 AND response_kind = ?2
                ORDER BY start_time DESC LIMIT 1",
                (recipe_id, ResponseStateKind::Success),
                |row| row.get(0),
            )
            .optional()
            .context("Error fetching request ID from database")
            .traced()
    }
}

/// Test-only helpers
#[cfg(test)]
impl RepositoryDatabase {
    /// Create an in-memory DB, only for testing
    pub fn testing() -> Self {
        let connection = Connection::open_in_memory().unwrap();
        let mut database = Self { connection };
        database.setup().unwrap();
        database
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
