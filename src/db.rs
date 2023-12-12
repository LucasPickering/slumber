//! The database is responsible for persisting data, including requests and
//! responses.

use crate::{
    collection::RequestRecipeId,
    http::{RequestId, RequestRecord},
    util::{Directory, ResultExt},
};
use anyhow::{anyhow, Context};
use derive_more::Display;
use rusqlite::{
    named_params,
    types::{FromSql, FromSqlError, FromSqlResult, ToSqlOutput, ValueRef},
    Connection, DatabaseName, OptionalExtension, Row, ToSql,
};
use rusqlite_migration::{Migrations, M};
use serde::{de::DeserializeOwned, Serialize};
use std::{
    fmt::Debug,
    ops::Deref,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tracing::{debug, info};
use uuid::Uuid;

/// A SQLite database for persisting data. Generally speaking, any error that
/// occurs *after* opening the DB connection should be an internal bug, but
/// should be shown to the user whenever possible. All operations are blocking,
/// to enable calling from the view code. Do not call on every frame though,
/// cache results in UI state for as long as they're needed.
///
/// There is only one database for an entire system. All collection share the
/// same DB, and can modify concurrently. Generally any data that is unique
/// to a collection should have an FK column to the `collections` table.
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

/// A unique ID for a collection. This is generated when the collection is
/// inserted into the DB.
#[derive(Copy, Clone, Debug, Display)]
pub struct CollectionId(Uuid);

impl Database {
    /// Load the database. This will perform first-time setup, so this should
    /// only be called at the main session entrypoint.
    pub fn load() -> anyhow::Result<Self> {
        let path = Self::path()?;
        info!(?path, "Loading database");
        let mut connection = Connection::open(path)?;
        connection.pragma_update(
            Some(DatabaseName::Main),
            "foreign_keys",
            "ON",
        )?;
        // Use WAL for concurrency
        connection.pragma_update(None, "journal_mode", "WAL")?;
        Self::migrate(&mut connection)?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
        })
    }

    /// Path to the database file. This will create the directory if it doesn't
    /// exist
    fn path() -> anyhow::Result<PathBuf> {
        Ok(Directory::root().create()?.join("state.sqlite"))
    }

    /// Apply database migrations
    fn migrate(connection: &mut Connection) -> anyhow::Result<()> {
        let migrations = Migrations::new(vec![
            M::up(
                // Path is the *canonicalzed* path to a collection file,
                // guaranteeing it will be stable and unique
                "CREATE TABLE collections (
                    id              UUID PRIMARY KEY NOT NULL,
                    path            BLOB NOT NULL UNIQUE
                )",
            )
            .down("DROP TABLE collections"),
            M::up(
                // The request state kind is a bit hard to map to tabular data.
                // Everything that we need to query on (HTTP status code,
                // end_time, etc.) is in its own column. Therequest/response
                // will be serialized into msgpack bytes
                "CREATE TABLE requests (
                    id              UUID PRIMARY KEY NOT NULL,
                    collection_id   UUID NOT NULL,
                    recipe_id       TEXT NOT NULL,
                    start_time      TEXT NOT NULL,
                    end_time        TEXT NOT NULL,
                    request         BLOB NOT NULL,
                    response        BLOB NOT NULL,
                    status_code     INTEGER NOT NULL,
                    FOREIGN KEY(collection_id) REFERENCES collections(id)
                )",
            )
            .down("DROP TABLE requests"),
            M::up(
                // Values will be serialized as msgpack
                "CREATE TABLE ui_state (
                    key             TEXT NOT NULL,
                    collection_id   UUID NOT NULL,
                    value           BLOB NOT NULL,
                    PRIMARY KEY (key, collection_id),
                    FOREIGN KEY(collection_id) REFERENCES collections(id)
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

    /// Get a list of all collections
    pub fn get_collections(&self) -> anyhow::Result<Vec<PathBuf>> {
        self.connection()
            .prepare("SELECT path FROM collections")?
            .query_map([], |row| Ok(row.get::<_, Bytes<_>>("path")?.0))
            .context("Error fetching collections")?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("Error extracting collection data")
    }

    /// Get a collection ID by path. Return an error if there is no collection
    /// with the given path
    pub fn get_collection_id(
        &self,
        path: &Path,
    ) -> anyhow::Result<CollectionId> {
        // Convert to canonicalize and make serializable
        let path: CollectionPath = path.try_into()?;

        self.connection()
            .query_row(
                "SELECT id FROM collections WHERE path = :path",
                named_params! {":path": &path},
                |row| row.get::<_, CollectionId>("id"),
            )
            .map_err(|err| match err {
                rusqlite::Error::QueryReturnedNoRows => {
                    // Use Display impl here because this will get shown in
                    // CLI output
                    anyhow!("Unknown collection `{path}`")
                }
                other => anyhow::Error::from(other)
                    .context("Error fetching collection ID"),
            })
            .traced()
    }

    /// Migrate all data for one collection into another, deleting the source
    /// collection
    pub fn merge_collections(
        &self,
        source: &Path,
        target: &Path,
    ) -> anyhow::Result<()> {
        info!(?source, ?target, "Merging database state");

        // Exchange each path for an ID
        let source = self.get_collection_id(source)?;
        let target = self.get_collection_id(target)?;

        // Update each table in individually
        let connection = self.connection();
        connection
            .execute(
                "UPDATE requests SET collection_id = :target
                WHERE collection_id = :source",
                named_params! {":source": source, ":target": target},
            )
            .context("Error migrating table `requests`")
            .traced()?;
        connection
            .execute(
                // Overwrite UI state. Maybe this isn't the best UX, but sqlite
                // doesn't provide an "UPDATE OR DELETE" so this is easiest and
                // still reasonable
                "UPDATE OR REPLACE ui_state SET collection_id = :target
                WHERE collection_id = :source",
                named_params! {":source": source, ":target": target},
            )
            .context("Error migrating table `ui_state`")
            .traced()?;

        connection
            .execute(
                "DELETE FROM collections WHERE id = :source",
                named_params! {":source": source},
            )
            .context("Error deleting source collection")
            .traced()?;
        Ok(())
    }

    /// Convert this database connection into a handle for a single collection
    /// file. This will store the collection in the DB if it isn't already,
    /// then grab its generated ID to create a [CollectionDatabase].
    pub fn into_collection(
        self,
        path: &Path,
    ) -> anyhow::Result<CollectionDatabase> {
        // Convert to canonicalize and make serializable
        let path: CollectionPath = path.try_into()?;

        // We have to set/get in two separate queries, because RETURNING doesn't
        // return anything if the insert didn't modify
        self.connection()
            .execute(
                "INSERT INTO collections (id, path) VALUES (:id, :path)
                ON CONFLICT(path) DO NOTHING",
                named_params! {
                    ":id": CollectionId(Uuid::new_v4()),
                    ":path": &path,
                },
            )
            .context("Error setting collection ID")
            .traced()?;
        let collection_id = self
            .connection()
            .query_row(
                "SELECT id FROM collections WHERE path = :path",
                named_params! {":path": &path},
                |row| row.get::<_, CollectionId>("id"),
            )
            .context("Error fetching collection ID")
            .traced()?;

        Ok(CollectionDatabase {
            collection_id,
            database: self,
        })
    }
}

/// A collection-specific database handle. This is a wrapper around a [Database]
/// that restricts all queries to a specific collection ID. Use
/// [Database::into_collection] to obtain one.
#[derive(Clone, Debug)]
pub struct CollectionDatabase {
    collection_id: CollectionId,
    database: Database,
}

impl CollectionDatabase {
    pub fn collection_id(&self) -> CollectionId {
        self.collection_id
    }

    /// Get the most recent request+response for a recipe, or `None` if there
    /// has never been one received.
    pub fn get_last_request(
        &self,
        recipe_id: &RequestRecipeId,
    ) -> anyhow::Result<Option<RequestRecord>> {
        self.database
            .connection()
            .query_row(
                "SELECT * FROM requests
                WHERE collection_id = :collection_id AND recipe_id = :recipe_id
                ORDER BY start_time DESC LIMIT 1",
                named_params! {
                    ":collection_id": self.collection_id,
                    ":recipe_id": recipe_id,
                },
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
            id = %record.id,
            url = %record.request.url,
            "Adding request record to database",
        );
        self.database
            .connection()
            .execute(
                "INSERT INTO
                requests (
                    id,
                    collection_id,
                    recipe_id,
                    start_time,
                    end_time,
                    request,
                    response,
                    status_code
                )
                VALUES (:id, :collection_id, :recipe_id, :start_time,
                    :end_time, :request, :response, :status_code)",
                named_params! {
                    ":id": record.id,
                    ":collection_id": self.collection_id,
                    ":recipe_id": &record.request.recipe_id,
                    ":start_time": &record.start_time,
                    ":end_time": &record.end_time,
                    ":request": &Bytes(&record.request),
                    ":response": &Bytes(&record.response),
                    ":status_code": record.response.status.as_u16(),
                },
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
            .database
            .connection()
            .query_row(
                "SELECT value FROM ui_state
                WHERE collection_id = :collection_id AND key = :key",
                named_params! {
                    ":collection_id": self.collection_id,
                    ":key": key.to_string(),
                },
                |row| {
                    let value: Bytes<V> = row.get("value")?;
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
        self.database
            .connection()
            .execute(
                // Upsert!
                "INSERT INTO ui_state (collection_id, key, value)
                VALUES (:collection_id, :key, :value)
                ON CONFLICT DO UPDATE SET value = excluded.value",
                named_params! {
                    ":collection_id": self.collection_id,
                    ":key": key.to_string(),
                    ":value": Bytes(value),
                },
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

/// Test-only helpers
#[cfg(test)]
impl CollectionDatabase {
    /// Create an in-memory DB, only for testing
    pub fn testing() -> Self {
        Database::testing()
            .into_collection(Path::new("./slumber.yml"))
            .expect("Error initializing DB collection")
    }
}

impl ToSql for CollectionId {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        self.0.to_sql()
    }
}

impl FromSql for CollectionId {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        Ok(Self(Uuid::column_result(value)?))
    }
}

impl ToSql for RequestId {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        self.0.to_sql()
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

/// Neat little wrapper for a collection path, to make sure it gets
/// canonicalized and serialized/deserialized consistently
#[derive(Debug, Display)]
#[display("{}", _0.0.display())]
struct CollectionPath(Bytes<PathBuf>);

impl TryFrom<&Path> for CollectionPath {
    type Error = anyhow::Error;

    fn try_from(path: &Path) -> Result<Self, Self::Error> {
        path.canonicalize()
            .context(format!("Error canonicalizing path {path:?}"))
            .traced()
            .map(|path| Self(Bytes(path)))
    }
}

impl ToSql for CollectionPath {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        self.0.to_sql()
    }
}

impl FromSql for CollectionPath {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        Bytes::<PathBuf>::column_result(value).map(Self)
    }
}

/// A wrapper to serialize/deserialize a value as msgpack for DB storage
#[derive(Debug)]
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::factory::*;
    use factori::create;

    #[test]
    fn test_merge() {
        let database = Database::testing();
        let path1 = Path::new("slumber.yml");
        let path2 = Path::new("README.md"); // Has to be a real file
        let collection1 = database.clone().into_collection(path1).unwrap();
        let collection2 = database.clone().into_collection(path2).unwrap();

        let record1 = create!(RequestRecord);
        let record2 = create!(RequestRecord);
        let recipe_id = &record1.request.recipe_id;
        let ui_key = "key1";
        collection1.insert_request(&record1).unwrap();
        collection1.set_ui(ui_key, "value1").unwrap();
        collection2.insert_request(&record2).unwrap();
        collection2.set_ui(ui_key, "value2").unwrap();

        // Sanity checks
        assert_eq!(
            collection1.get_last_request(recipe_id).unwrap().unwrap().id,
            record1.id
        );
        assert_eq!(
            collection1.get_ui::<_, String>(ui_key).unwrap(),
            Some("value1".into())
        );
        assert_eq!(
            collection2.get_last_request(recipe_id).unwrap().unwrap().id,
            record2.id
        );
        assert_eq!(
            collection2.get_ui::<_, String>(ui_key).unwrap(),
            Some("value2".into())
        );

        // Do the merge
        database.merge_collections(path2, path1).unwrap();

        // Collection 2 values should've overwritten
        assert_eq!(
            collection1.get_last_request(recipe_id).unwrap().unwrap().id,
            record2.id
        );
        assert_eq!(
            collection1.get_ui::<_, String>(ui_key).unwrap(),
            Some("value2".into())
        );

        // Make sure collection2 was deleted
        assert_eq!(
            database.get_collections().unwrap(),
            vec![path1.canonicalize().unwrap()]
        );
    }

    /// Test request storage and retrieval
    #[test]
    fn test_request() {
        let database = Database::testing();
        let collection1 = database
            .clone()
            .into_collection(Path::new("slumber.yml"))
            .unwrap();
        let collection2 = database
            .clone()
            .into_collection(Path::new("README.md"))
            .unwrap();

        let record1 = create!(RequestRecord);
        let record2 = create!(RequestRecord);
        collection1.insert_request(&record1).unwrap();
        collection2.insert_request(&record2).unwrap();

        // Make sure the two have a conflicting recipe ID, which should be
        // de-conflicted via the collection ID
        assert_eq!(record1.request.recipe_id, record2.request.recipe_id);
        let recipe_id = &record1.request.recipe_id;

        assert_eq!(
            collection1.get_last_request(recipe_id).unwrap().unwrap().id,
            record1.id
        );
        assert_eq!(
            collection2.get_last_request(recipe_id).unwrap().unwrap().id,
            record2.id
        );
    }

    /// Test UI state storage and retrieval
    #[test]
    fn test_ui_state() {
        let database = Database::testing();
        let collection1 = database
            .clone()
            .into_collection(Path::new("slumber.yml"))
            .unwrap();
        let collection2 = database
            .clone()
            .into_collection(Path::new("README.md"))
            .unwrap();

        let ui_key = "key1";
        collection1.set_ui(ui_key, "value1").unwrap();
        collection2.set_ui(ui_key, "value2").unwrap();

        assert_eq!(
            collection1.get_ui::<_, String>(ui_key).unwrap(),
            Some("value1".into())
        );
        assert_eq!(
            collection2.get_ui::<_, String>(ui_key).unwrap(),
            Some("value2".into())
        );
    }
}
