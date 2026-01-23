//! The database is responsible for persisting data, including requests and
//! responses.

mod convert;
mod migrations;
#[cfg(test)]
mod tests;

use crate::{
    collection::{Collection, CollectionFile, ProfileId, RecipeId},
    database::convert::{CollectionPath, SqlWrap},
    http::{Exchange, ExchangeSummary, RequestId},
};
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, named_params};
use slumber_util::{ResultTraced, paths};
use std::{
    borrow::Cow,
    fmt::Debug,
    io,
    ops::{Deref, DerefMut},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use thiserror::Error;
use tracing::{debug, info, trace};
use uuid::Uuid;

/// Maximum number of commands to store in history **per collection**. When we
/// hit the cap, the oldest commands get evicted.
const MAX_COMMAND_HISTORY_SIZE: u32 = 100;

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
/// Schema is defined in `migrations`
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
    pub fn load() -> Result<Self, DatabaseError> {
        Self::from_path(&Self::path())
    }

    /// [Self::load], but from a particular data directory. Useful only for
    /// tests, when the default DB path shouldn't be used.
    pub fn from_directory(directory: &Path) -> Result<Self, DatabaseError> {
        let path = directory.join(Self::FILE);
        Self::from_path(&path)
    }

    fn from_path(path: &Path) -> Result<Self, DatabaseError> {
        paths::create_parent(path).map_err(DatabaseError::Directory)?;

        info!(?path, "Loading database");
        let mut connection = Connection::open(path)
            .and_then(|conn| {
                conn.pragma_update(None, "foreign_keys", "ON")?;
                // Use WAL for concurrency
                conn.pragma_update(None, "journal_mode", "WAL")?;
                Ok(conn)
            })
            .map_err(DatabaseError::add_context("Opening database"))?;
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
    fn migrate(connection: &mut Connection) -> Result<(), DatabaseError> {
        migrations::migrations()
            .to_latest(connection)
            .map_err(DatabaseError::Migrate)
    }

    /// Get a reference to the DB connection. Panics if the lock is poisoned
    fn connection(&self) -> impl '_ + DerefMut<Target = Connection> {
        self.connection.lock().expect("Connection lock poisoned")
    }

    /// Get a list of all collections
    pub fn get_collections(
        &self,
    ) -> Result<Vec<CollectionMetadata>, DatabaseError> {
        // Wish we had try blocks ¯\_(ツ)_/¯
        self.connection()
            .prepare("SELECT * FROM collections")
            .and_then(|mut stmt| {
                stmt.query_map([], |row| row.try_into())?
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .map_err(DatabaseError::add_context("Querying collections"))
            .traced()
    }

    /// Get a collection's ID by its path. This will canonicalize the path to
    /// ensure it matches what we've stored in the DB. Return an error if the
    /// path isn't in the DB.
    pub fn get_collection_id(
        &self,
        path: &Path,
    ) -> Result<CollectionId, DatabaseError> {
        // Convert to canonicalize and make serializable
        let path = CollectionPath::try_from_path_maybe_missing(path)?;

        self.connection()
            .query_row(
                "SELECT id FROM collections WHERE path = :path",
                named_params! {":path": &path},
                |row| row.get::<_, CollectionId>("id"),
            )
            .map_err(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => {
                    DatabaseError::ResourceUnknown {
                        kind: "collection",
                        id: path.to_string(),
                    }
                }
                _ => {
                    DatabaseError::with_context(error, "Querying collection ID")
                }
            })
            .traced()
    }

    /// Get metadata about a collection from its ID
    pub fn get_collection_metadata(
        &self,
        id: CollectionId,
    ) -> Result<CollectionMetadata, DatabaseError> {
        self.connection()
            .query_row(
                "SELECT * FROM collections WHERE id = :id",
                named_params! {":id": id},
                |row| CollectionMetadata::try_from(row),
            )
            .map_err(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => {
                    DatabaseError::ResourceUnknown {
                        kind: "collection",
                        id: id.to_string(),
                    }
                }
                _ => DatabaseError::with_context(
                    error,
                    "Querying collection metadata",
                ),
            })
            .traced()
    }

    /// Delete a collection from the DB, including all requests and other rows
    /// associated with it
    pub fn delete_collection(
        &self,
        collection: CollectionId,
    ) -> Result<(), DatabaseError> {
        // Delete all rows referencing the collection before deleting the
        // collection. It would be nice to use `ON DELETE CASCADE`, but you can
        // only set that when the table is created. Sqlite doesn't allowing
        // creating or modifying foreign keys on a table once it's created.
        // Manually deleting everything is simpler than writing a huge migration
        // just to make this automatic

        let statements = [
            "DELETE FROM requests_v2 WHERE collection_id = :id",
            "DELETE FROM ui_state_v2 WHERE collection_id = :id",
            "DELETE FROM commands WHERE collection_id = :id",
            "DELETE FROM collections WHERE id = :id",
        ];

        let mut connection = self.connection();
        connection
            .transaction()
            .and_then(|tx| {
                for statement in statements {
                    tx.prepare(statement)?
                        .execute(named_params! {":id": collection})?;
                }
                tx.commit()?;
                Ok(())
            })
            .map_err({
                DatabaseError::add_context(format!(
                    "Deleting collection `{collection}`"
                ))
            })
            .traced()
    }

    /// Migrate all data for one collection into another, deleting the source
    /// collection
    pub fn merge_collections(
        &self,
        source: CollectionId,
        target: CollectionId,
    ) -> Result<(), DatabaseError> {
        info!(?source, ?target, "Merging database state");
        let mut connection = self.connection();
        let tx = connection.transaction()?;

        // Update each table in individually
        tx.execute(
            "UPDATE requests_v2 SET collection_id = :target
                WHERE collection_id = :source",
            named_params! {":source": source, ":target": target},
        )
        .map_err(DatabaseError::add_context("Merging table `requests_v2`"))
        .traced()?;
        tx.execute(
            // Overwrite UI state. Maybe this isn't the best UX, but sqlite
            // doesn't provide an "UPDATE OR DELETE" so this is easiest and
            // still reasonable
            "UPDATE OR REPLACE ui_state_v2 SET collection_id = :target
                WHERE collection_id = :source",
            named_params! {":source": source, ":target": target},
        )
        .map_err(DatabaseError::add_context("Merging table `ui_state_v2`"))
        .traced()?;
        tx.execute(
            // Overwrite command history. If there's an overlap, we'll take the
            // timestamp from the source collection because it's easiest.
            // Sqlite doesn't have an "UPDATE OR DELETE"
            "UPDATE OR REPLACE commands SET collection_id = :target
                WHERE collection_id = :source",
            named_params! {":source": source, ":target": target},
        )
        .map_err(DatabaseError::add_context("Merging table `commands`"))
        .traced()?;

        // Delete the collection now that nothing is referencing it
        tx.execute(
            "DELETE FROM collections WHERE id = :source",
            named_params! {":source": source},
        )
        .map_err(DatabaseError::add_context("Deleting source collection"))
        .traced()?;
        tx.commit()?;

        Ok(())
    }

    /// Get all requests for all collections
    pub fn get_all_requests(
        &self,
    ) -> Result<Vec<ExchangeSummary>, DatabaseError> {
        // Wish we had try blocks ¯\_(ツ)_/¯
        self.connection()
            .prepare(
                "SELECT id, recipe_id, profile_id, start_time, end_time,
                    status_code FROM requests_v2 ORDER BY start_time DESC",
            )
            .and_then(|mut stmt| {
                stmt.query_map((), |row| row.try_into())?
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .map_err(DatabaseError::add_context("Querying requests"))
            .traced()
    }

    /// Delete a single exchange by ID. Return the number of deleted requests
    pub fn delete_request(
        &self,
        request_id: RequestId,
    ) -> Result<usize, DatabaseError> {
        trace!(%request_id, "Deleting request");
        self.connection()
            .execute(
                "DELETE FROM requests_v2 WHERE id = :request_id",
                named_params! {":request_id": request_id},
            )
            .map_err({
                DatabaseError::add_context(format!(
                    "Deleting request `{request_id}`"
                ))
            })
            .traced()
    }

    /// Convert this database connection into a handle for a single collection
    /// file. This will store the collection in the DB if it isn't already,
    /// then grab its generated ID to create a [CollectionDatabase].
    pub fn into_collection(
        self,
        file: &CollectionFile,
    ) -> Result<CollectionDatabase, DatabaseError> {
        // Convert to canonicalize and make serializable
        let path = CollectionPath::try_from_path(file.path())?;

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
            .map_err(DatabaseError::add_context("Setting collection ID"))
            .traced()?;
        let collection_id = self
            .connection()
            .query_row(
                "SELECT id FROM collections WHERE path = :path",
                named_params! {":path": &path},
                |row| row.get::<_, CollectionId>("id"),
            )
            .map_err(DatabaseError::add_context("Querying collection ID"))
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
    /// Get metadata for the collection associated with this DB handle
    pub fn metadata(&self) -> Result<CollectionMetadata, DatabaseError> {
        self.database
            .connection()
            .query_row(
                "SELECT * FROM collections WHERE id = :id",
                named_params! {":id": self.collection_id},
                |row| row.try_into(),
            )
            .map_err(DatabaseError::add_context("Querying collection path"))
            .traced()
    }

    /// Get the root database, which has access to all collections
    pub fn root(&self) -> &Database {
        &self.database
    }

    /// Store the collection's display name in the DB
    ///
    /// This should be set
    /// whenever the collection file is loaded to ensure it's up to date in
    /// the database. If this fails it will log the result, but not return it.
    /// There's nothing meaningful for the caller to do with the result
    /// beyond log it again.
    pub fn set_name(&self, collection: &Collection) {
        let name = collection.name.as_deref();
        let _ = self
            .database
            .connection()
            .execute(
                "UPDATE collections SET name = :name WHERE id = :id",
                named_params! {":id": self.collection_id, ":name": name},
            )
            .map_err(DatabaseError::add_context("Updating collection name"))
            .traced();
    }

    /// Get a request by ID, or `None` if it does not exist in history.
    pub fn get_request(
        &self,
        request_id: RequestId,
    ) -> Result<Option<Exchange>, DatabaseError> {
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
            .map_err(DatabaseError::add_context(format!(
                "Querying request {request_id} from database"
            )))
            .traced()
    }

    /// Get the most recent request+response (by start time) for a profile +
    /// recipe, or `None` if there has never been one received. If the given
    /// profile is `None`, match all requests that have no associated profile.
    pub fn get_latest_request(
        &self,
        profile_id: ProfileFilter,
        recipe_id: &RecipeId,
    ) -> Result<Option<Exchange>, DatabaseError> {
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
            .map_err(DatabaseError::add_context(format!(
                "Querying request [profile={profile_id:?}; \
                    recipe={recipe_id}] from database"
            )))
            .traced()
    }

    /// Get a list of all requests for a profile+recipe combo
    pub fn get_recipe_requests(
        &self,
        profile_filter: ProfileFilter,
        recipe_id: &RecipeId,
    ) -> Result<Vec<ExchangeSummary>, DatabaseError> {
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
            .map_err(DatabaseError::add_context(
                "Querying request history from database",
            ))
            .traced()?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(DatabaseError::add_context("Extracting request history"))
    }

    /// Get all requests for this collection
    pub fn get_all_requests(
        &self,
    ) -> Result<Vec<ExchangeSummary>, DatabaseError> {
        trace!("Fetching requests for collection");
        self.database
            .connection()
            .prepare(
                "SELECT id, recipe_id, profile_id, start_time, end_time,
                    status_code FROM requests_v2
                WHERE collection_id = :collection_id ORDER BY start_time DESC",
            )
            .and_then(|mut stmt| {
                stmt.query_map(
                    named_params! {":collection_id": self.collection_id},
                    |row| row.try_into(),
                )?
                .collect::<rusqlite::Result<Vec<_>>>()
            })
            .map_err(DatabaseError::add_context(format!(
                "Querying all requests for collection `{}`",
                self.collection_id
            )))
            .traced()
    }

    /// Add a new exchange to history. The HTTP engine is responsible for
    /// inserting its own exchanges. Only requests that received a valid HTTP
    /// response should be stored. In-flight requests, invalid requests, and
    /// requests that failed to complete (e.g. because of a network error)
    /// should not (and cannot) be stored.
    pub fn insert_exchange(
        &self,
        exchange: &Exchange,
    ) -> Result<(), DatabaseError> {
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
            .map_err({
                DatabaseError::add_context(format!(
                    "Inserting request `{}`",
                    exchange.id
                ))
            })
            .traced()?;
        Ok(())
    }

    /// Delete all requests for a recipe+profile combo. Return the IDs of the
    /// deleted requests
    pub fn delete_recipe_requests(
        &self,
        profile_id: ProfileFilter,
        recipe_id: &RecipeId,
    ) -> Result<Vec<RequestId>, DatabaseError> {
        info!(
            collection = ?self.metadata(),
            %recipe_id,
            ?profile_id,
            "Deleting all requests for recipe+profile",
        );
        self.database
            .connection()
            .prepare(
                "DELETE FROM requests_v2 WHERE collection_id = :collection_id
                    AND (:ignore_profile_id OR profile_id IS :profile_id)
                    AND recipe_id = :recipe_id
                RETURNING id",
            )
            .and_then(|mut stmt| {
                stmt.query_map(
                    named_params! {
                        ":collection_id": self.collection_id,
                        // `IS` needed for profile_id so `None` will match
                        // `NULL`. We want to dynamically ignore the profile
                        // filter if the user is asking for all profiles.
                        // Dynamically modifying the query is really ugly so the
                        // easiest thing is to use an additional parameter to
                        // bypass the filter
                        ":ignore_profile_id": profile_id == ProfileFilter::All,
                        ":profile_id": profile_id,
                        ":recipe_id": recipe_id,
                    },
                    |row| row.get::<_, RequestId>("id"),
                )?
                .collect::<rusqlite::Result<Vec<_>>>()
            })
            .map_err({
                DatabaseError::add_context(format!(
                    "Deleting requests for recipe `{recipe_id}`"
                ))
            })
            .traced()
    }

    /// Delete a single request by ID
    pub fn delete_request(
        &self,
        request_id: RequestId,
    ) -> Result<(), DatabaseError> {
        info!(
            collection = ?self.metadata(),
            %request_id,
            "Deleting request"
        );
        self.database
            .connection()
            .execute(
                "DELETE FROM requests_v2 WHERE id = :request_id",
                named_params! {":request_id": request_id},
            )
            .map_err({
                DatabaseError::add_context(format!(
                    "Deleting request `{request_id}`"
                ))
            })
            .traced()?;
        Ok(())
    }

    /// Get the value of a UI state field. Key type is included as part of the
    /// key, to disambiguate between keys of identical structure
    pub fn get_ui(
        &self,
        key_type: &str,
        key: &str,
    ) -> Result<Option<String>, DatabaseError> {
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
                    ":key": &key,
                },
                |row| row.get("value"),
            )
            .optional()
            .map_err(DatabaseError::add_context(format!(
                "Querying UI state key `{key:?}`"
            )))
            .traced()?;
        trace!(?key, ?value, "Fetched UI state");
        Ok(value)
    }

    /// Set the value of a UI state field
    pub fn set_ui(
        &self,
        key_type: &str,
        key: &str,
        value: &str,
    ) -> Result<(), DatabaseError> {
        trace!(?key, ?value, "Setting UI state");
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
                    ":key": &key,
                    ":value": value,
                },
            )
            .map_err({
                DatabaseError::add_context(format!(
                    "Inserting UI state key `{key:?}`"
                ))
            })
            .traced()?;
        Ok(())
    }

    /// Insert a query/export command into the command history table. Commands
    /// are deduped in history, so if it's already in the table, just update
    /// the timestamp on it
    pub fn insert_command(&self, command: &str) -> Result<(), DatabaseError> {
        debug!(?command, "Storing query/export command");

        // Shitty try block
        let deleted = (|| {
            let connection = self.database.connection();
            connection.execute(
                // Holy fuck it's an upsert!!
                // Also, delete any commands beyond the max size
                "INSERT INTO commands (collection_id, command, time)
                VALUES (:collection_id, :command, :time)
                ON CONFLICT DO UPDATE SET time = :time",
                named_params! {
                    ":collection_id": self.collection_id,
                    ":command": command,
                    ":time": Utc::now(),
                },
            )?;
            // Delete oldest commands beyond max size. Cap is PER COLLECTION!!
            //
            // SQLite supports LIMIT/OFFSET directly in the DELETE which would
            // work here, but it requires a compile-time option to be enabled.
            // The CTE is just easier that fucking around with that.
            // https://sqlite.org/compile.html#enable_update_delete_limit
            connection.execute(
                "WITH to_delete AS
                    (SELECT command FROM commands WHERE
                        collection_id = :collection_id
                        ORDER BY time DESC
                        LIMIT -1 OFFSET :max_history_size)
                DELETE FROM commands WHERE command IN to_delete",
                named_params! {
                    ":collection_id": self.collection_id,
                    ":max_history_size": MAX_COMMAND_HISTORY_SIZE
                },
            )
        })()
        .map_err({
            DatabaseError::add_context(format!(
                "Inserting command `{command:?}`"
            ))
        })
        .traced()?;

        if deleted > 0 {
            debug!("Evicted {deleted} rows from `commands` table");
        }

        Ok(())
    }

    /// Get historical query/export commands that start with the given prefix.
    /// Results will be ordered descending by their most recent execution time
    /// (most recent commands first).
    pub fn get_commands(
        &self,
        prefix: &str,
    ) -> Result<Vec<String>, DatabaseError> {
        trace!(prefix, "Getting commands from history matching prefix");
        self.database
            .connection()
            .prepare(
                "SELECT command FROM commands
                WHERE collection_id = :collection_id
                    AND command LIKE :prefix || '%'
                ORDER BY time DESC",
            )?
            .query_map(
                named_params! {
                    ":collection_id": self.collection_id,
                    ":prefix": prefix,
                },
                |row| row.get("command"),
            )
            .map_err(DatabaseError::add_context(format!(
                "Querying commands with prefix `{prefix}`",
            )))
            .and_then(|cursor| {
                cursor.collect::<rusqlite::Result<Vec<_>>>().map_err(
                    DatabaseError::add_context("Extracting command history"),
                )
            })
            .traced()
    }

    /// Get a command from the command history table
    ///
    /// ## Params
    ///
    /// - `offset`: Index of the command to grab, with 0 being the most recent
    ///   and moving back in time from there
    /// - `exclude`: Command string to exclude from the results. The command
    ///   prompt excludes the current text from results to prevent duplicates.
    pub fn get_command(
        &self,
        offset: u32,
        exclude: &str,
    ) -> Result<Option<String>, DatabaseError> {
        trace!(offset, "Getting command from history with offset");
        self.database
            .connection()
            .query_row(
                "SELECT command FROM commands
                WHERE collection_id = :collection_id AND command != :exclude
                ORDER BY time DESC
                LIMIT 1 OFFSET :offset",
                named_params! {
                    ":collection_id": self.collection_id,
                    ":offset": offset,
                    ":exclude": exclude,
                },
                |row| row.get("command"),
            )
            .optional()
            .map_err(DatabaseError::add_context("Querying commands"))
            .traced()
    }

    /// Get the unique ID of this collection
    pub fn collection_id(&self) -> CollectionId {
        self.collection_id
    }
}

/// A unique ID for a collection. This is generated when the collection is
/// inserted into the DB.
#[derive(
    Copy, Clone, Debug, derive_more::Display, derive_more::FromStr, PartialEq,
)]
#[cfg_attr(any(test, feature = "test"), derive(Eq, Hash))]
pub struct CollectionId(Uuid);

impl CollectionId {
    fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

/// Info about a collection from the database
#[derive(Clone, Debug)]
#[cfg_attr(any(test, feature = "test"), derive(PartialEq))]
pub struct CollectionMetadata {
    pub id: CollectionId,
    pub path: PathBuf,
    pub name: Option<String>,
}

impl CollectionMetadata {
    /// Get the name if available, otherwise fall back to the path
    pub fn display_name(&self) -> String {
        self.name
            .clone()
            .unwrap_or_else(|| self.path.display().to_string())
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

/// Convert from a specific profile
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

/// Convert from a specific profile
impl From<ProfileId> for ProfileFilter<'static> {
    fn from(profile_id: ProfileId) -> Self {
        Self::Some(Cow::Owned(profile_id))
    }
}

/// Convert from an option that defines either *no* profile or a specific one
impl From<Option<ProfileId>> for ProfileFilter<'static> {
    fn from(value: Option<ProfileId>) -> Self {
        match value {
            Some(profile_id) => Self::Some(Cow::Owned(profile_id)),
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

/// Any error that can occur while accessing the local database
#[derive(Debug, Error)]
pub enum DatabaseError {
    /// An error with additional context attached
    #[error("{context}")]
    Context { context: String, error: Box<Self> },

    /// Error creating the parent directory of the DB file
    #[error("Error creating data directory")]
    Directory(#[source] io::Error),

    /// Error applying migrations to the DBs
    #[error(transparent)]
    Migrate(rusqlite_migration::Error),

    /// Error getting the path for a collection file
    #[error("Getting collection path `{}`", path.display())]
    Path {
        path: PathBuf,
        #[source]
        error: io::Error,
    },

    /// Queried for some resource by a unique identifier, but it wasn't found
    /// in the DB
    #[error("Unknown {kind} `{id}`")]
    ResourceUnknown { kind: &'static str, id: String },

    /// Any SQL error. Generally this should be wrapped in a [Self::Context]
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
}

impl DatabaseError {
    /// Create a function that will attach the given context to an error.
    /// Convenient to pass to [Result::map_err].
    pub fn add_context<Ctx, E>(context: Ctx) -> impl FnOnce(E) -> Self
    where
        Ctx: Into<String>,
        E: Into<Self>,
    {
        move |error| Self::Context {
            context: context.into(),
            error: Box::new(error.into()),
        }
    }

    /// Attach context to an error error
    pub fn with_context<Ctx, E>(error: E, context: Ctx) -> Self
    where
        Ctx: Into<String>,
        E: Into<Self>,
    {
        Self::Context {
            context: context.into(),
            error: Box::new(error.into()),
        }
    }
}
