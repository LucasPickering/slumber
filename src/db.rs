//! The database is responsible for persisting data, including requests and
//! responses.

use crate::{
    collection::{ProfileId, RecipeId},
    http::{RequestId, RequestRecord},
    util::{
        paths::{DataDirectory, FileGuard},
        ResultExt,
    },
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
#[cfg_attr(test, derive(Eq, Hash, PartialEq))]
pub struct CollectionId(Uuid);

impl Database {
    const FILE: &'static str = "state.sqlite";

    /// Load the database. This will perform migrations, but can be called from
    /// anywhere in the app. The migrations will run on first connection, and
    /// not after that.
    pub fn load() -> anyhow::Result<Self> {
        let path = Self::path().create_parent()?;
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

    /// Path to the database file
    pub fn path() -> FileGuard {
        DataDirectory::root().file(Self::FILE)
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
                    profile_id      TEXT,
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
                // keys+values will be serialized as msgpack
                "CREATE TABLE ui_state (
                    key             BLOB NOT NULL,
                    collection_id   UUID NOT NULL,
                    value           BLOB NOT NULL,
                    PRIMARY KEY (key, collection_id),
                    FOREIGN KEY(collection_id) REFERENCES collections(id)
                )",
            )
            .down("DROP TABLE ui_state"),
            // This is a sledgehammer migration. Added when we switch from
            // rmp_serde::to_vec to rmp_serde::to_vec_named. This affected the
            // serialization of all binary blobs, so there's no easy way to
            // migrate it all. It's easiest just to wipe it all out.
            M::up("DELETE FROM requests; DELETE FROM ui_state;").down(""),
        ]);
        migrations.to_latest(connection)?;
        Ok(())
    }

    /// Get a reference to the DB connection. Panics if the lock is poisoned
    fn connection(&self) -> impl '_ + Deref<Target = Connection> {
        self.connection.lock().expect("Connection lock poisoned")
    }

    /// Get a list of all collections
    pub fn collections(&self) -> anyhow::Result<Vec<PathBuf>> {
        self.connection()
            .prepare("SELECT path FROM collections")?
            .query_map([], |row| Ok(row.get::<_, ByteEncoded<_>>("path")?.0))
            .context("Error fetching collections")?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("Error extracting collection data")
    }

    /// Migrate all data for one collection into another, deleting the source
    /// collection
    pub fn merge_collections(
        &self,
        source: &Path,
        target: &Path,
    ) -> anyhow::Result<()> {
        fn get_collection_id(
            connection: &Connection,
            path: &Path,
        ) -> anyhow::Result<CollectionId> {
            // Convert to canonicalize and make serializable
            let path: CollectionPath = path.try_into()?;

            connection
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

        info!(?source, ?target, "Merging database state");
        let connection = self.connection();

        // Exchange each path for an ID
        let source = get_collection_id(&connection, source)?;
        let target = get_collection_id(&connection, target)?;

        // Update each table in individually
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

    /// Get the full path for the collection file associated with this DB handle
    pub fn collection_path(&self) -> anyhow::Result<PathBuf> {
        self.database
            .connection()
            .query_row(
                "SELECT path FROM collections WHERE id = :id",
                named_params! {":id": self.collection_id},
                |row| row.get::<_, CollectionPath>("path"),
            )
            .context("Error fetching collection path")
            .traced()
            .map(PathBuf::from)
    }

    /// Get the most recent request+response for a profile+recipe, or `None` if
    /// there has never been one received. If the given profile is `None`, match
    /// all requests that have no associated profile.
    pub fn get_last_request(
        &self,
        profile_id: Option<&ProfileId>,
        recipe_id: &RecipeId,
    ) -> anyhow::Result<Option<RequestRecord>> {
        self.database
            .connection()
            .query_row(
                // `IS` needed for profile_id so `None` will match `NULL`
                "SELECT * FROM requests
                WHERE collection_id = :collection_id
                    AND profile_id IS :profile_id
                    AND recipe_id = :recipe_id
                ORDER BY start_time DESC LIMIT 1",
                named_params! {
                    ":collection_id": self.collection_id,
                    ":profile_id": profile_id,
                    ":recipe_id": recipe_id,
                },
                |row| row.try_into(),
            )
            .optional()
            .with_context(|| {
                format!(
                    "Error fetching request [profile={}; recipe={}] \
                    from database",
                    profile_id.map(ProfileId::to_string).unwrap_or_default(),
                    recipe_id
                )
            })
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
                    profile_id,
                    recipe_id,
                    start_time,
                    end_time,
                    request,
                    response,
                    status_code
                )
                VALUES (:id, :collection_id, :profile_id, :recipe_id,
                    :start_time, :end_time, :request, :response, :status_code)",
                named_params! {
                    ":id": record.id,
                    ":collection_id": self.collection_id,
                    ":profile_id": &record.request.profile_id,
                    ":recipe_id": &record.request.recipe_id,
                    ":start_time": &record.start_time,
                    ":end_time": &record.end_time,
                    ":request": &ByteEncoded(&*record.request),
                    ":response": &ByteEncoded(&*record.response),
                    ":status_code": record.response.status.as_u16(),
                },
            )
            .context(format!("Error saving request {} to database", record.id))
            .traced()?;
        Ok(())
    }

    /// Get the value of a UI state field
    pub fn get_ui<K, V>(&self, key: K) -> anyhow::Result<Option<V>>
    where
        K: Debug + Serialize,
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
                    ":key": ByteEncoded(&key),
                },
                |row| {
                    let value: ByteEncoded<V> = row.get("value")?;
                    Ok(value.0)
                },
            )
            .optional()
            .context(format!("Error fetching UI state for {key:?}"))
            .traced()?;
        debug!(?key, ?value, "Fetched UI state");
        Ok(value)
    }

    /// Set the value of a UI state field
    pub fn set_ui<K, V>(&self, key: K, value: V) -> anyhow::Result<()>
    where
        K: Debug + Serialize,
        V: Debug + Serialize,
    {
        debug!(?key, ?value, "Setting UI state");
        self.database
            .connection()
            .execute(
                // Upsert!
                "INSERT INTO ui_state (collection_id, key, value)
                VALUES (:collection_id, :key, :value)
                ON CONFLICT DO UPDATE SET value = excluded.value",
                named_params! {
                    ":collection_id": self.collection_id,
                    ":key": ByteEncoded(key),
                    ":value": ByteEncoded(value),
                },
            )
            .context("Error saving UI state to database")
            .traced()?;
        Ok(())
    }
}

/// Create an in-memory DB, only for testing
#[cfg(test)]
impl crate::test_util::Factory for Database {
    fn factory() -> Self {
        let mut connection = Connection::open_in_memory().unwrap();
        Self::migrate(&mut connection).unwrap();
        Self {
            connection: Arc::new(Mutex::new(connection)),
        }
    }
}

/// Create an in-memory DB, only for testing
#[cfg(test)]
impl crate::test_util::Factory for CollectionDatabase {
    fn factory() -> Self {
        Database::factory()
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

impl ToSql for ProfileId {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        self.deref().to_sql()
    }
}

impl FromSql for ProfileId {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        Ok(String::column_result(value)?.into())
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

impl ToSql for RecipeId {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        self.deref().to_sql()
    }
}

impl FromSql for RecipeId {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        Ok(String::column_result(value)?.into())
    }
}

/// Neat little wrapper for a collection path, to make sure it gets
/// canonicalized and serialized/deserialized consistently
#[derive(Debug, Display)]
#[display("{}", _0.0.display())]
struct CollectionPath(ByteEncoded<PathBuf>);

impl From<CollectionPath> for PathBuf {
    fn from(path: CollectionPath) -> Self {
        path.0 .0
    }
}

impl TryFrom<&Path> for CollectionPath {
    type Error = anyhow::Error;

    fn try_from(path: &Path) -> Result<Self, Self::Error> {
        path.canonicalize()
            .context(format!("Error canonicalizing path {path:?}"))
            .traced()
            .map(|path| Self(ByteEncoded(path)))
    }
}

impl ToSql for CollectionPath {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        self.0.to_sql()
    }
}

impl FromSql for CollectionPath {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        ByteEncoded::<PathBuf>::column_result(value).map(Self)
    }
}

/// A wrapper to serialize/deserialize a value as msgpack for DB storage
#[derive(Debug)]
struct ByteEncoded<T>(T);

impl<T: Serialize> ToSql for ByteEncoded<T> {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        let bytes = rmp_serde::to_vec_named(&self.0).map_err(|err| {
            rusqlite::Error::ToSqlConversionFailure(Box::new(err))
        })?;
        Ok(ToSqlOutput::Owned(bytes.into()))
    }
}

impl<T: DeserializeOwned> FromSql for ByteEncoded<T> {
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
            request: Arc::new(row.get::<_, ByteEncoded<_>>("request")?.0),
            response: Arc::new(row.get::<_, ByteEncoded<_>>("response")?.0),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{http::Request, test_util::*};
    use std::collections::HashMap;

    #[test]
    fn test_merge() {
        let database = Database::factory();
        let path1 = Path::new("slumber.yml");
        let path2 = Path::new("README.md"); // Has to be a real file
        let collection1 = database.clone().into_collection(path1).unwrap();
        let collection2 = database.clone().into_collection(path2).unwrap();

        let record1 = RequestRecord::factory();
        let record2 = RequestRecord::factory();
        let profile_id = record1.request.profile_id.as_ref();
        let recipe_id = &record1.request.recipe_id;
        let ui_key = "key1";
        collection1.insert_request(&record1).unwrap();
        collection1.set_ui(ui_key, "value1").unwrap();
        collection2.insert_request(&record2).unwrap();
        collection2.set_ui(ui_key, "value2").unwrap();

        // Sanity checks
        assert_eq!(
            collection1
                .get_last_request(profile_id, recipe_id)
                .unwrap()
                .unwrap()
                .id,
            record1.id
        );
        assert_eq!(
            collection1.get_ui::<_, String>(ui_key).unwrap(),
            Some("value1".into())
        );
        assert_eq!(
            collection2
                .get_last_request(profile_id, recipe_id)
                .unwrap()
                .unwrap()
                .id,
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
            collection1
                .get_last_request(profile_id, recipe_id)
                .unwrap()
                .unwrap()
                .id,
            record2.id
        );
        assert_eq!(
            collection1.get_ui::<_, String>(ui_key).unwrap(),
            Some("value2".into())
        );

        // Make sure collection2 was deleted
        assert_eq!(
            database.collections().unwrap(),
            vec![path1.canonicalize().unwrap()]
        );
    }

    /// Test request storage and retrieval
    #[test]
    fn test_request() {
        let database = Database::factory();
        let collection1 = database
            .clone()
            .into_collection(Path::new("slumber.yml"))
            .unwrap();
        let collection2 = database
            .clone()
            .into_collection(Path::new("README.md"))
            .unwrap();

        let record2 = RequestRecord::factory();
        collection2.insert_request(&record2).unwrap();

        // We separate requests by 3 columns. Create multiple of each column to
        // make sure we filter by each column correctly
        let collections = [collection1, collection2];

        // Store the created request ID for each cell in the matrix, so we can
        // compare to what the DB spits back later
        let mut request_ids: HashMap<
            (CollectionId, Option<ProfileId>, RecipeId),
            RequestId,
        > = Default::default();

        // Create and insert each request
        for collection in &collections {
            for profile_id in [None, Some("profile1"), Some("profile2")] {
                for recipe_id in ["recipe1", "recipe2"] {
                    let recipe_id: RecipeId = recipe_id.into();
                    let profile_id = profile_id.map(ProfileId::from);
                    let request = Request {
                        profile_id: profile_id.clone(),
                        recipe_id: recipe_id.clone(),
                        ..Request::factory()
                    };
                    let record = RequestRecord {
                        request: request.into(),
                        ..RequestRecord::factory()
                    };
                    collection.insert_request(&record).unwrap();
                    request_ids.insert(
                        (collection.collection_id(), profile_id, recipe_id),
                        record.id,
                    );
                }
            }
        }

        // Try to find each inserted recipe individually. Also try some
        // expected non-matches
        for collection in &collections {
            for profile_id in [None, Some("profile1"), Some("extra_profile")] {
                for recipe_id in ["recipe1", "extra_recipe"] {
                    let collection_id = collection.collection_id();
                    let profile_id = profile_id.map(ProfileId::from);
                    let recipe_id = recipe_id.into();

                    // Leave the Option here so a non-match will trigger a handy
                    // assertion error
                    let record_id = collection
                        .get_last_request(profile_id.as_ref(), &recipe_id)
                        .unwrap()
                        .map(|record| record.id);
                    let expected_id = request_ids.get(&(
                        collection_id,
                        profile_id.clone(),
                        recipe_id.clone(),
                    ));

                    assert_eq!(
                        record_id.as_ref(),
                        expected_id,
                        "Request mismatch for collection = {collection_id}, \
                        profile = {profile_id:?}, recipe = {recipe_id}"
                    );
                }
            }
        }
    }

    /// Test UI state storage and retrieval
    #[test]
    fn test_ui_state() {
        let database = Database::factory();
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
