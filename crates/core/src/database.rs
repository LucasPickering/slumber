//! The database is responsible for persisting data, including requests and
//! responses.

mod convert;
mod migrations;
#[cfg(test)]
mod tests;

use crate::{
    collection::{CollectionFile, ProfileId, RecipeId},
    database::convert::{CollectionPath, JsonEncoded, SqlWrap},
    http::{Exchange, ExchangeSummary, RequestId},
};
use anyhow::{Context, anyhow};
use derive_more::Display;
use rusqlite::{Connection, DatabaseName, OptionalExtension, named_params};
use serde::{Serialize, de::DeserializeOwned};
use slumber_util::{ResultTraced, paths};
use std::{
    borrow::Cow,
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
        Self::from_path(&Self::path())
    }

    /// [Self::load], but from a particular data directory. Useful only for
    /// tests, when the default DB path shouldn't be used.
    pub fn from_directory(directory: &Path) -> anyhow::Result<Self> {
        let path = directory.join(Self::FILE);
        Self::from_path(&path)
    }

    fn from_path(path: &Path) -> anyhow::Result<Self> {
        paths::create_parent(path)?;

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

    /// Get all requests for all collections
    pub fn get_all_requests(&self) -> anyhow::Result<Vec<ExchangeSummary>> {
        trace!("Fetching requests for all collections");
        self.connection()
            .prepare(
                "SELECT id, recipe_id, profile_id, start_time, end_time,
                    status_code FROM requests_v2 ORDER BY start_time DESC",
            )?
            .query_map((), |row| row.try_into())
            .context("Error fetching request history from database")
            .traced()?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("Error extracting request history")
    }

    /// Delete a single exchange by ID. Return the number of deleted requests
    pub fn delete_request(
        &self,
        request_id: RequestId,
    ) -> anyhow::Result<usize> {
        info!(%request_id, "Deleting request");

        self.connection()
            .execute(
                "DELETE FROM requests_v2 WHERE id = :request_id",
                named_params! {":request_id": request_id},
            )
            .context(format!("Error deleting request {request_id}"))
            .traced()
    }

    /// Convert this database connection into a handle for a single collection
    /// file. This will store the collection in the DB if it isn't already,
    /// then grab its generated ID to create a [CollectionDatabase].
    pub fn into_collection(
        self,
        file: &CollectionFile,
    ) -> anyhow::Result<CollectionDatabase> {
        // Convert to canonicalize and make serializable
        let path: CollectionPath = file.path().try_into()?;

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
                format!("Error fetching request {request_id} from database")
            })
            .traced()
    }

    /// Get the most recent request+response (by start time) for a profile +
    /// recipe, or `None` if there has never been one received. If the given
    /// profile is `None`, match all requests that have no associated profile.
    pub fn get_latest_request(
        &self,
        profile_id: ProfileFilter,
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
                // `IS` needed for profile_id so `None` will match `NULL`.
                // We want to dynamically ignore the profile filter if the user
                // is asking for all profiles. Dynamically modifying the query
                // is really ugly so the easiest thing is to use an additional
                // parameter to bypass the filter
                "SELECT * FROM requests_v2
                WHERE collection_id = :collection_id
                    AND (:ignore_profile_id OR profile_id IS :profile_id)
                    AND recipe_id = :recipe_id
                ORDER BY start_time DESC LIMIT 1",
                named_params! {
                    ":collection_id": self.collection_id,
                    ":ignore_profile_id": profile_id == ProfileFilter::All,
                    ":profile_id": profile_id,
                    ":recipe_id": recipe_id,
                },
                |row| row.try_into(),
            )
            .optional()
            .with_context(|| {
                format!(
                    "Error fetching request [profile={profile_id:?}; \
                    recipe={recipe_id}] from database"
                )
            })
            .traced()
    }

    /// Get a list of all requests for a profile+recipe combo
    pub fn get_recipe_requests(
        &self,
        profile_filter: ProfileFilter,
        recipe_id: &RecipeId,
    ) -> anyhow::Result<Vec<ExchangeSummary>> {
        trace!(
            profile_id = ?profile_filter,
            recipe_id = %recipe_id,
            "Fetching requests from database"
        );
        self.database
            .connection()
            .prepare(
                // `IS` needed for profile_id so `None` will match `NULL`.
                // We want to dynamically ignore the profile filter if the user
                // is asking for all profiles. Dynamically modifying the query
                // is really ugly so the easiest thing is to use an additional
                // parameter to bypass the filter
                "SELECT id, recipe_id, profile_id, start_time, end_time,
                    status_code FROM requests_v2
                WHERE collection_id = :collection_id
                    AND (:ignore_profile_id OR profile_id IS :profile_id)
                    AND recipe_id = :recipe_id
                ORDER BY start_time DESC",
            )?
            .query_map(
                named_params! {
                    ":collection_id": self.collection_id,
                    ":ignore_profile_id": profile_filter == ProfileFilter::All,
                    ":profile_id": profile_filter,
                    ":recipe_id": recipe_id,
                },
                |row| row.try_into(),
            )
            .context("Error fetching request history from database")
            .traced()?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("Error extracting request history")
    }

    /// Get all requests for this collection
    pub fn get_all_requests(&self) -> anyhow::Result<Vec<ExchangeSummary>> {
        trace!("Fetching requests for collection");
        self.database
            .connection()
            .prepare(
                "SELECT id, recipe_id, profile_id, start_time, end_time,
                    status_code FROM requests_v2
                WHERE collection_id = :collection_id ORDER BY start_time DESC",
            )?
            .query_map(
                named_params! {":collection_id": self.collection_id},
                |row| row.try_into(),
            )
            .context("Error fetching request history from database")
            .traced()?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("Error extracting request history")
    }

    /// Add a new exchange to history. The HTTP engine is responsible for
    /// inserting its own exchanges. Only requests that received a valid HTTP
    /// response should be stored. In-flight requests, invalid requests, and
    /// requests that failed to complete (e.g. because of a network error)
    /// should not (and cannot) be stored.
    pub fn insert_exchange(&self, exchange: &Exchange) -> anyhow::Result<()> {
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
                    http_version,
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
                    :http_version,
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

                    ":http_version": exchange.request.http_version,
                    ":method": exchange.request.method,
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

    /// Delete all requests for a recipe+profile combo. Return the number of
    /// deleted requests
    pub fn delete_recipe_requests(
        &self,
        profile_id: ProfileFilter,
        recipe_id: &RecipeId,
    ) -> anyhow::Result<usize> {
        info!(
            collection_id = %self.collection_id,
            collection_path = ?self.collection_path(),
            %recipe_id,
            ?profile_id,
            "Deleting all requests for recipe+profile",
        );
        self.database
            .connection()
            .execute(
                // `IS` needed for profile_id so `None` will match `NULL`.
                // We want to dynamically ignore the profile filter if the user
                // is asking for all profiles. Dynamically modifying the query
                // is really ugly so the easiest thing is to use an additional
                // parameter to bypass the filter
                "DELETE FROM requests_v2 WHERE collection_id = :collection_id
                    AND (:ignore_profile_id OR profile_id IS :profile_id)
                    AND recipe_id = :recipe_id",
                named_params! {
                    ":collection_id": self.collection_id,
                    ":ignore_profile_id": profile_id == ProfileFilter::All,
                    ":profile_id": profile_id,
                    ":recipe_id": recipe_id,
                },
            )
            .context("Error deleting requests")
            .traced()
    }

    /// Delete a single request by ID
    pub fn delete_request(&self, request_id: RequestId) -> anyhow::Result<()> {
        info!(
            collection_id = %self.collection_id,
            collection_path = ?self.collection_path(),
            %request_id,
            "Deleting request"
        );
        self.database
            .connection()
            .execute(
                "DELETE FROM requests_v2 WHERE id = :request_id",
                named_params! {":request_id": request_id},
            )
            .context(format!("Error deleting request {request_id}"))
            .traced()?;
        Ok(())
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

#[cfg(any(test, feature = "test"))]
impl slumber_util::Factory for Database {
    fn factory((): ()) -> Self {
        let mut connection = Connection::open_in_memory().unwrap();
        Self::migrate(&mut connection).unwrap();
        Self {
            connection: Arc::new(Mutex::new(connection)),
        }
    }
}

#[cfg(any(test, feature = "test"))]
impl slumber_util::Factory for CollectionDatabase {
    fn factory((): ()) -> Self {
        use slumber_util::paths::get_repo_root;
        Database::factory(())
            .into_collection(
                &CollectionFile::new(Some(get_repo_root().join("slumber.yml")))
                    .unwrap(),
            )
            .expect("Error initializing DB collection")
    }
}

/// Define how to filter requests by profile
#[derive(Clone, Debug, Default, PartialEq)]
pub enum ProfileFilter<'a> {
    /// Show requests with _no_ associated profile
    None,
    /// Show requests for a particular profile
    Some(Cow<'a, ProfileId>),
    /// Show requests for all profiles
    #[default]
    All,
}

impl ProfileFilter<'_> {
    /// Does the profile ID match this filter?
    pub fn matches(&self, profile_id: Option<&ProfileId>) -> bool {
        match self {
            Self::None => profile_id.is_none(),
            Self::Some(expected) => profile_id == Some(expected),
            Self::All => true,
        }
    }

    /// Get a `'static` copy of this filter. If the filter is a borrowed profile
    /// ID, it will be cloned
    pub fn into_owned(self) -> ProfileFilter<'static> {
        match self {
            Self::None => ProfileFilter::None,
            Self::Some(profile_id) => {
                ProfileFilter::Some(Cow::Owned(profile_id.into_owned()))
            }
            Self::All => ProfileFilter::All,
        }
    }
}

/// Convert from an option that defines either *no* profile or a specific one
impl<'a> From<&'a ProfileId> for ProfileFilter<'a> {
    fn from(profile_id: &'a ProfileId) -> Self {
        Self::Some(Cow::Borrowed(profile_id))
    }
}

/// Convert from an option that defines either *no* profile or a specific one
impl<'a> From<Option<&'a ProfileId>> for ProfileFilter<'a> {
    fn from(value: Option<&'a ProfileId>) -> Self {
        match value {
            Some(profile_id) => Self::Some(Cow::Borrowed(profile_id)),
            None => Self::None,
        }
    }
}

/// Useful for CLI arguments
impl From<Option<Option<ProfileId>>> for ProfileFilter<'static> {
    fn from(value: Option<Option<ProfileId>>) -> Self {
        match value {
            Some(Some(profile_id)) => Self::Some(Cow::Owned(profile_id)),
            Some(None) => Self::None,
            None => Self::All,
        }
    }
}
