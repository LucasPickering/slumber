//! The database is responsible for persisting data, including requests and
//! responses.

mod convert;
mod migrations;

use crate::{
    collection::{ProfileId, RecipeId},
    db::convert::{CollectionPath, JsonEncoded, SqlWrap},
    http::{Exchange, ExchangeSummary, RequestId},
    util::{paths, ResultTraced},
};
use anyhow::{anyhow, Context};
use derive_more::Display;
use rusqlite::{named_params, Connection, DatabaseName, OptionalExtension};
use serde::{de::DeserializeOwned, Serialize};
use std::{
    fmt::Debug,
    ops::{Deref, DerefMut},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tracing::{debug, info, trace};
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
///
/// Schema is defined in [migrations]
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
    const FILE: &'static str = "state.sqlite";

    /// Load the database. This will perform migrations, but can be called from
    /// anywhere in the app. The migrations will run on first connection, and
    /// not after that.
    pub fn load() -> anyhow::Result<Self> {
        let path = Self::path();
        paths::create_parent(&path)?;

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
    pub fn path() -> PathBuf {
        paths::data_directory().join(Self::FILE)
    }

    /// Apply database migrations
    fn migrate(connection: &mut Connection) -> anyhow::Result<()> {
        migrations::migrations().to_latest(connection)?;
        Ok(())
    }

    /// Get a reference to the DB connection. Panics if the lock is poisoned
    fn connection(&self) -> impl '_ + DerefMut<Target = Connection> {
        self.connection.lock().expect("Connection lock poisoned")
    }

    /// Get a list of all collections
    pub fn collections(&self) -> anyhow::Result<Vec<PathBuf>> {
        self.connection()
            .prepare("SELECT path FROM collections")?
            .query_map([], |row| {
                Ok(row.get::<_, CollectionPath>("path")?.into())
            })
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
                "UPDATE requests_v2 SET collection_id = :target
                WHERE collection_id = :source",
                named_params! {":source": source, ":target": target},
            )
            .context("Error migrating table `requests_v2`")
            .traced()?;
        connection
            .execute(
                // Overwrite UI state. Maybe this isn't the best UX, but sqlite
                // doesn't provide an "UPDATE OR DELETE" so this is easiest and
                // still reasonable
                "UPDATE OR REPLACE ui_state_v2 SET collection_id = :target
                WHERE collection_id = :source",
                named_params! {":source": source, ":target": target},
            )
            .context("Error migrating table `ui_state_v2`")
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
        mode: DatabaseMode,
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
                    ":id": CollectionId::new(),
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
            mode,
        })
    }
}

/// A collection-specific database handle. This is a wrapper around a [Database]
/// that restricts all queries to a specific collection ID. Use
/// [Database::into_collection] to obtain one. You can freely clone this.
#[derive(Clone, Debug)]
pub struct CollectionDatabase {
    collection_id: CollectionId,
    database: Database,
    mode: DatabaseMode,
}

impl CollectionDatabase {
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

    /// Is read/write mode enabled for this database?
    pub fn can_write(&self) -> bool {
        self.mode == DatabaseMode::ReadWrite
    }

    /// Return an error if we are in read-only mode
    fn ensure_write(&self) -> anyhow::Result<()> {
        if self.can_write() {
            Ok(())
        } else {
            Err(anyhow!("Database in read-only mode"))
        }
    }

    /// Get a request by ID, or `None` if it does not exist in history.
    pub fn get_request(
        &self,
        request_id: RequestId,
    ) -> anyhow::Result<Option<Exchange>> {
        trace!(request_id = %request_id, "Fetching request from database");
        self.database
            .connection()
            .query_row(
                "SELECT * FROM requests_v2
                WHERE collection_id = :collection_id
                    AND id = :request_id
                ORDER BY start_time DESC LIMIT 1",
                named_params! {
                    // Include collection ID just to be extra safe
                    ":collection_id": self.collection_id,
                    ":request_id": request_id,
                },
                |row| row.try_into(),
            )
            .optional()
            .with_context(|| {
                format!("Error fetching request {} from database", request_id)
            })
            .traced()
    }

    /// Get the most recent request+response (by start time) for a profile +
    /// recipe, or `None` if there has never been one received. If the given
    /// profile is `None`, match all requests that have no associated profile.
    pub fn get_latest_request(
        &self,
        profile_id: Option<&ProfileId>,
        recipe_id: &RecipeId,
    ) -> anyhow::Result<Option<Exchange>> {
        trace!(
            profile_id = ?profile_id,
            recipe_id = %recipe_id,
            "Fetching last request from database"
        );
        self.database
            .connection()
            .query_row(
                // `IS` needed for profile_id so `None` will match `NULL`
                "SELECT * FROM requests_v2
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

    /// Add a new exchange to history. The HTTP engine is responsible for
    /// inserting its own exchanges. Only requests that received a valid HTTP
    /// response should be stored. In-flight requests, invalid requests, and
    /// requests that failed to complete (e.g. because of a network error)
    /// should not (and cannot) be stored.
    pub fn insert_exchange(&self, exchange: &Exchange) -> anyhow::Result<()> {
        self.ensure_write()?;

        debug!(
            id = %exchange.id,
            url = %exchange.request.url,
            "Adding exchange to database",
        );
        self.database
            .connection()
            .execute(
                "INSERT INTO
                requests_v2 (
                    id,
                    collection_id,
                    profile_id,
                    recipe_id,
                    start_time,
                    end_time,
                    method,
                    url,
                    request_headers,
                    request_body,
                    status_code,
                    response_headers,
                    response_body
                )
                VALUES (
                    :id,
                    :collection_id,
                    :profile_id,
                    :recipe_id,
                    :start_time,
                    :end_time,
                    :method,
                    :url,
                    :request_headers,
                    :request_body,
                    :status_code,
                    :response_headers,
                    :response_body
                )",
                named_params! {
                    ":id": exchange.id,
                    ":collection_id": self.collection_id,
                    ":profile_id": &exchange.request.profile_id,
                    ":recipe_id": &exchange.request.recipe_id,
                    ":start_time": &exchange.start_time,
                    ":end_time": &exchange.end_time,

                    ":method": exchange.request.method.as_str(),
                    ":url": exchange.request.url.as_str(),
                    ":request_headers": SqlWrap(&exchange.request.headers),
                    ":request_body": exchange.request.body(),

                    ":status_code": exchange.response.status.as_u16(),
                    ":response_headers": SqlWrap(&exchange.response.headers),
                    ":response_body": exchange.response.body.bytes().deref(),
                },
            )
            .context(format!(
                "Error saving request {} to database",
                exchange.id
            ))
            .traced()?;
        Ok(())
    }

    /// Get all requests for a recipe, across all profiles
    pub fn get_all_requests(
        &self,
        recipe_id: &RecipeId,
    ) -> anyhow::Result<Vec<ExchangeSummary>> {
        trace!(

            recipe_id = %recipe_id,
            "Fetching request history from database"
        );
        self.database
            .connection()
            .prepare(
                "SELECT id, profile_id, start_time, end_time, status_code
                FROM requests_v2
                WHERE collection_id = :collection_id AND recipe_id = :recipe_id
                ORDER BY start_time DESC",
            )?
            .query_map(
                named_params! {
                    ":collection_id": self.collection_id,
                    ":recipe_id": recipe_id,
                },
                |row| row.try_into(),
            )
            .context("Error fetching request history from database")
            .traced()?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("Error extracting request history")
    }

    /// Get a list of all requests for a profile+recipe combo
    pub fn get_profile_requests(
        &self,
        profile_id: Option<&ProfileId>,
        recipe_id: &RecipeId,
    ) -> anyhow::Result<Vec<ExchangeSummary>> {
        trace!(
            profile_id = ?profile_id,
            recipe_id = %recipe_id,
            "Fetching request history from database"
        );
        // It would be nice to de-dupe this code with get_all_requests, but
        // there's no good way to dynamically build a query with sqlite so it
        // ends up not being worth it
        self.database
            .connection()
            .prepare(
                "SELECT id, profile_id, start_time, end_time, status_code
                FROM requests_v2
                WHERE collection_id = :collection_id
                    AND profile_id IS :profile_id
                    AND recipe_id = :recipe_id
                ORDER BY start_time DESC",
            )?
            .query_map(
                named_params! {
                    ":collection_id": self.collection_id,
                    ":profile_id": profile_id,
                    ":recipe_id": recipe_id,
                },
                |row| row.try_into(),
            )
            .context("Error fetching request history from database")
            .traced()?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("Error extracting request history")
    }

    /// Get the value of a UI state field. Key type is included as part of the
    /// key, to disambiguate between keys of identical structure
    pub fn get_ui<K, V>(
        &self,
        key_type: &str,
        key: K,
    ) -> anyhow::Result<Option<V>>
    where
        K: Debug + Serialize,
        V: Debug + DeserializeOwned,
    {
        let value = self
            .database
            .connection()
            .query_row(
                "SELECT value FROM ui_state_v2
                WHERE collection_id = :collection_id
                    AND key_type = :key_type
                    AND key = :key",
                named_params! {
                    ":collection_id": self.collection_id,
                    ":key_type": key_type,
                    ":key": JsonEncoded(&key),
                },
                |row| {
                    let value: JsonEncoded<V> = row.get("value")?;
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
    pub fn set_ui<K, V>(
        &self,
        key_type: &str,
        key: K,
        value: V,
    ) -> anyhow::Result<()>
    where
        K: Debug + Serialize,
        V: Debug + Serialize,
    {
        self.ensure_write()?;

        debug!(?key, ?value, "Setting UI state");
        self.database
            .connection()
            .execute(
                // Upsert!
                "INSERT INTO ui_state_v2 (collection_id, key_type, key, value)
                VALUES (:collection_id, :key_type, :key, :value)
                ON CONFLICT DO UPDATE SET value = excluded.value",
                named_params! {
                    ":collection_id": self.collection_id,
                    ":key_type": key_type,
                    ":key": JsonEncoded(key),
                    ":value": JsonEncoded(value),
                },
            )
            .context("Error saving UI state to database")
            .traced()?;
        Ok(())
    }

    #[cfg(test)]
    pub fn collection_id(&self) -> CollectionId {
        self.collection_id
    }
}

/// A unique ID for a collection. This is generated when the collection is
/// inserted into the DB.
#[derive(Copy, Clone, Debug, Display)]
#[cfg_attr(test, derive(Eq, Hash, PartialEq))]
pub struct CollectionId(Uuid);

impl CollectionId {
    fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

/// Create an in-memory DB, only for testing
#[cfg(any(test, feature = "test"))]
impl crate::test_util::Factory for Database {
    fn factory(_: ()) -> Self {
        let mut connection = Connection::open_in_memory().unwrap();
        Self::migrate(&mut connection).unwrap();
        Self {
            connection: Arc::new(Mutex::new(connection)),
        }
    }
}

/// Create an in-memory DB, only for testing
#[cfg(any(test, feature = "test"))]
impl crate::test_util::Factory for CollectionDatabase {
    fn factory(_: ()) -> Self {
        Self::factory(DatabaseMode::ReadWrite)
    }
}

#[cfg(any(test, feature = "test"))]
impl crate::test_util::Factory<DatabaseMode> for CollectionDatabase {
    fn factory(mode: DatabaseMode) -> Self {
        use crate::util::paths::get_repo_root;
        Database::factory(())
            .into_collection(&get_repo_root().join("slumber.yml"), mode)
            .expect("Error initializing DB collection")
    }
}

/// Is the database read-only or read/write?
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum DatabaseMode {
    ReadOnly,
    ReadWrite,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{assert_err, test_util::Factory, util::paths::get_repo_root};
    use itertools::Itertools;
    use std::collections::HashMap;

    #[test]
    fn test_merge() {
        let database = Database::factory(());
        let path1 = get_repo_root().join("slumber.yml");
        let path2 = get_repo_root().join("README.md"); // Has to be a real file
        let collection1 = database
            .clone()
            .into_collection(&path1, DatabaseMode::ReadWrite)
            .unwrap();
        let collection2 = database
            .clone()
            .into_collection(&path2, DatabaseMode::ReadWrite)
            .unwrap();

        let exchange1 =
            Exchange::factory((Some("profile1".into()), "recipe1".into()));
        let exchange2 =
            Exchange::factory((Some("profile1".into()), "recipe1".into()));
        let profile_id = exchange1.request.profile_id.as_ref();
        let recipe_id = &exchange1.request.recipe_id;
        let key_type = "MyKey";
        let ui_key = "key1";
        collection1.insert_exchange(&exchange1).unwrap();
        collection1.set_ui(key_type, ui_key, "value1").unwrap();
        collection2.insert_exchange(&exchange2).unwrap();
        collection2.set_ui(key_type, ui_key, "value2").unwrap();

        // Sanity checks
        assert_eq!(
            collection1
                .get_latest_request(profile_id, recipe_id)
                .unwrap()
                .unwrap()
                .id,
            exchange1.id
        );
        assert_eq!(
            collection1.get_ui::<_, String>(key_type, ui_key).unwrap(),
            Some("value1".into())
        );
        assert_eq!(
            collection2
                .get_latest_request(profile_id, recipe_id)
                .unwrap()
                .unwrap()
                .id,
            exchange2.id
        );
        assert_eq!(
            collection2.get_ui::<_, String>(key_type, ui_key).unwrap(),
            Some("value2".into())
        );

        // Do the merge
        database.merge_collections(&path2, &path1).unwrap();

        // Collection 2 values should've overwritten
        assert_eq!(
            collection1
                .get_latest_request(profile_id, recipe_id)
                .unwrap()
                .unwrap()
                .id,
            exchange2.id
        );
        assert_eq!(
            collection1.get_ui::<_, String>(key_type, ui_key).unwrap(),
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
        let database = Database::factory(());
        let collection1 = database
            .clone()
            .into_collection(
                &get_repo_root().join("slumber.yml"),
                DatabaseMode::ReadWrite,
            )
            .unwrap();
        let collection2 = database
            .clone()
            .into_collection(
                &get_repo_root().join("README.md"),
                DatabaseMode::ReadWrite,
            )
            .unwrap();

        let exchange2 = Exchange::factory(());
        collection2.insert_exchange(&exchange2).unwrap();

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
                    let exchange = Exchange::factory((
                        profile_id.clone(),
                        recipe_id.clone(),
                    ));
                    collection.insert_exchange(&exchange).unwrap();
                    request_ids.insert(
                        (collection.collection_id(), profile_id, recipe_id),
                        exchange.id,
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
                    let exchange_id = collection
                        .get_latest_request(profile_id.as_ref(), &recipe_id)
                        .unwrap()
                        .map(|exchange| exchange.id);
                    let expected_id = request_ids.get(&(
                        collection_id,
                        profile_id.clone(),
                        recipe_id.clone(),
                    ));

                    assert_eq!(
                        exchange_id.as_ref(),
                        expected_id,
                        "Request mismatch for collection = {collection_id}, \
                        profile = {profile_id:?}, recipe = {recipe_id}"
                    );
                }
            }
        }
    }

    #[test]
    fn test_load_all_requests() {
        let database = CollectionDatabase::factory(());

        // Create and insert multiple requests per profile+recipe.
        // Store the created request ID for each cell in the matrix, so we can
        // compare to what the DB spits back later
        let mut request_ids: HashMap<
            (Option<ProfileId>, RecipeId),
            Vec<RequestId>,
        > = Default::default();
        for profile_id in [None, Some("profile1"), Some("profile2")] {
            for recipe_id in ["recipe1", "recipe2"] {
                let recipe_id: RecipeId = recipe_id.into();
                let profile_id = profile_id.map(ProfileId::from);
                let mut ids = (0..3)
                    .map(|_| {
                        let exchange = Exchange::factory((
                            profile_id.clone(),
                            recipe_id.clone(),
                        ));
                        database.insert_exchange(&exchange).unwrap();
                        exchange.id
                    })
                    .collect_vec();
                // Order newest->oldest, that's the response we expect
                ids.reverse();
                request_ids.insert((profile_id, recipe_id), ids);
            }
        }

        // Try to find each inserted recipe individually. Also try some
        // expected non-matches
        for profile_id in [None, Some("profile1"), Some("extra_profile")] {
            for recipe_id in ["recipe1", "extra_recipe"] {
                let profile_id = profile_id.map(ProfileId::from);
                let recipe_id = recipe_id.into();

                // Leave the Option here so a non-match will trigger a handy
                // assertion error
                let ids = database
                    .get_profile_requests(profile_id.as_ref(), &recipe_id)
                    .unwrap()
                    .into_iter()
                    .map(|exchange| exchange.id)
                    .collect_vec();
                let expected_id = request_ids
                    .get(&(profile_id.clone(), recipe_id.clone()))
                    .cloned()
                    .unwrap_or_default();

                assert_eq!(
                    ids, expected_id,
                    "Requests mismatch for \
                    profile = {profile_id:?}, recipe = {recipe_id}"
                );
            }
        }

        // Load all requests for a recipe (across all profiles)
        let recipe_id = "recipe1".into();
        let ids = database
            .get_all_requests(&recipe_id)
            .unwrap()
            .into_iter()
            .map(|exchange| exchange.id)
            .sorted()
            .collect_vec();
        let expected_ids = request_ids
            .iter()
            .filter(|((_, r), _)| r == &recipe_id)
            .flat_map(|(_, request_ids)| request_ids)
            .sorted()
            .copied()
            .collect_vec();
        assert_eq!(ids, expected_ids)
    }

    /// Test UI state storage and retrieval
    #[test]
    fn test_ui_state() {
        let database = Database::factory(());
        let collection1 = database
            .clone()
            .into_collection(
                Path::new("../../slumber.yml"),
                DatabaseMode::ReadWrite,
            )
            .unwrap();
        let collection2 = database
            .clone()
            .into_collection(Path::new("Cargo.toml"), DatabaseMode::ReadWrite)
            .unwrap();

        let key_type = "MyKey";
        let ui_key = "key1";
        collection1.set_ui(key_type, ui_key, "value1").unwrap();
        collection2.set_ui(key_type, ui_key, "value2").unwrap();

        assert_eq!(
            collection1.get_ui::<_, String>(key_type, ui_key).unwrap(),
            Some("value1".into())
        );
        assert_eq!(
            collection2.get_ui::<_, String>(key_type, ui_key).unwrap(),
            Some("value2".into())
        );
    }

    #[test]
    fn test_readonly_mode() {
        let database = CollectionDatabase::factory(DatabaseMode::ReadOnly);
        assert_err!(
            database.insert_exchange(&Exchange::factory(())),
            "Database in read-only mode"
        );
        assert_err!(
            database.set_ui("MyKey", "key1", "value1"),
            "Database in read-only mode"
        );
    }
}
