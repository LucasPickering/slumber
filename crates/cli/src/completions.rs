//! Shell completion utilities
//!
//! To test this locally:
//! - `cargo install --path .` (current version of Slumber must be in $PATH)
//! - `COMPLETE=<shell> slumber` and pipe that to `source`
//!
//! That will enable completions for the current shell

use clap_complete::{
    ArgValueCompleter, CompletionCandidate, PathCompleter,
    engine::ValueCompleter,
};
use slumber_core::{
    collection::{Collection, CollectionError, CollectionFile, ProfileId},
    database::Database,
};
use std::{ffi::OsStr, ops::Deref};
use tracing::level_filters::LevelFilter;

/// Build a completer for profile IDs from the default collection
pub fn complete_profile() -> ArgValueCompleter {
    ArgValueCompleter::new(|current: &OsStr| {
        load_collection()
            .map(|collection| {
                get_candidates(
                    collection.profiles.keys().map(ProfileId::to_string),
                    current,
                )
            })
            .unwrap_or_default()
    })
}

/// Build a completer for recipe IDs from the default collection
pub fn complete_recipe() -> ArgValueCompleter {
    ArgValueCompleter::new(|current: &OsStr| {
        load_collection()
            .map(|collection| get_recipe_ids(&collection, current))
            .unwrap_or_default()
    })
}

/// Build a completer for recipe IDs from the default collection OR request IDs
/// from the DB
pub fn complete_recipe_or_request_id() -> ArgValueCompleter {
    ArgValueCompleter::new(move |current: &OsStr| {
        let mut completions = Vec::new();

        // Suggest recipe IDs *first* because they're probably more useful
        completions.extend(
            load_collection()
                .map(|collection| get_recipe_ids(&collection, current))
                .unwrap_or_default(),
        );

        // Suggestion request IDs
        completions.extend(
            Database::load()
                .and_then(|db| db.get_all_requests())
                .map(|exchanges| {
                    get_candidates(
                        exchanges
                            .into_iter()
                            .map(|exchange| exchange.id.to_string()),
                        current,
                    )
                })
                .unwrap_or_default(),
        );

        completions
    })
}

/// Build a completer for `.yml` and `.yaml` files
pub fn complete_collection_path() -> ArgValueCompleter {
    ArgValueCompleter::new(collection_path_completer())
}

/// Build a completer for collection IDs from the DB *and* YAML paths
///
/// DB can be provided for tests
pub fn complete_collection_specifier() -> ArgValueCompleter {
    let path_completer = collection_path_completer();
    ArgValueCompleter::new(move |current: &OsStr| {
        let mut completions = Vec::new();
        // Suggest paths *first* because they're probably more useful
        completions.extend(path_completer.complete(current));
        // Suggest all matching collection IDs
        completions.extend(
            Database::load()
                .and_then(|db| db.get_collections())
                .map(|collections| {
                    // Get all matching collection IDs
                    get_candidates(
                        collections
                            .into_iter()
                            .map(|collection| collection.id.to_string()),
                        current,
                    )
                })
                .unwrap_or_default(),
        );
        completions
    })
}

/// Complete --log-level
pub fn complete_log_level() -> ArgValueCompleter {
    ArgValueCompleter::new(|current: &OsStr| {
        get_candidates(
            [
                LevelFilter::OFF,
                LevelFilter::ERROR,
                LevelFilter::WARN,
                LevelFilter::INFO,
                LevelFilter::DEBUG,
                LevelFilter::TRACE,
            ]
            .into_iter()
            .map(|l| l.to_string()),
            current,
        )
    })
}

/// Load the default collection
///
/// For now we just lean on the default collection paths. In the future we
/// should be able to look for a --file arg in the command and use that path,
/// but clap doesn't support that yet
/// <https://github.com/clap-rs/clap/issues/5784>
fn load_collection() -> Result<Collection, CollectionError> {
    let collection_file = CollectionFile::new(None)?;
    collection_file.load()
}

/// Get a completer for YAML files
fn collection_path_completer() -> PathCompleter {
    PathCompleter::file().filter(|path| {
        let extension = path.extension();
        extension == Some(OsStr::new("yml"))
            || extension == Some(OsStr::new("yaml"))
    })
}

/// Find matching recipe IDs in the collection
fn get_recipe_ids(
    collection: &Collection,
    current: &OsStr,
) -> Vec<CompletionCandidate> {
    get_candidates(
        collection
            .recipes
            .iter()
            // Include recipe IDs only. Folder IDs are never passed
            // to the CLI
            .filter_map(|(_, node)| Some(node.recipe()?.id.to_string())),
        current,
    )
}

/// Get all iterms in the iterator that match the given prefix, returning them
/// as [CompletionCandidate]s
fn get_candidates<T: Into<String>>(
    iter: impl Iterator<Item = T>,
    current: &OsStr,
) -> Vec<CompletionCandidate> {
    let Some(current) = current.to_str() else {
        return Vec::new();
    };
    // Only include IDs prefixed by the input we've gotten so far
    iter.map(T::into)
        .filter(|value| value.starts_with(current))
        .map(|value| CompletionCandidate::new(value.deref()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use env_lock::CurrentDirGuard;
    use rstest::{fixture, rstest};
    use slumber_core::http::{Exchange, RequestId};
    use slumber_util::{DataDir, Factory, data_dir};
    use std::path::{Path, PathBuf};

    /// Complete profile IDs from the collection
    #[rstest]
    fn test_complete_profile(_current_dir: CurrentDirGuard) {
        let completions = complete(complete_profile());
        assert_eq!(&completions, &["profile1", "profile2"]);
    }

    /// Complete recipe IDs from the collection
    #[rstest]
    fn test_complete_recipe(_current_dir: CurrentDirGuard) {
        let completions = complete(complete_recipe());
        assert_eq!(
            &completions,
            &[
                "getUser",
                "query",
                "headers",
                "authBasic",
                "authBearer",
                "textBody",
                "jsonBody",
                "fileBody",
                "urlencoded",
                "multipart",
                "chained",
                "override",
            ]
        );
    }

    /// Complete recipe IDs from the collection OR request IDs from the database
    #[rstest]
    fn test_complete_recipe_or_request_id(
        _current_dir: CurrentDirGuard,
        database: TestDatabase,
    ) {
        // Put two requests in the DB
        let id1 = RequestId::new();
        let id2 = RequestId::new();
        let collection_db = database
            .database
            .into_collection(&collection_file())
            .unwrap();
        for id in [id1, id2] {
            collection_db
                .insert_exchange(&Exchange::factory(id))
                .unwrap();
        }

        let completions = complete(complete_recipe_or_request_id());
        assert_eq!(
            &completions,
            &[
                "getUser",
                "query",
                "headers",
                "authBasic",
                "authBearer",
                "textBody",
                "jsonBody",
                "fileBody",
                "urlencoded",
                "multipart",
                "chained",
                "override",
                &id2.to_string(),
                &id1.to_string()
            ]
        );
    }

    /// Complete YAML file paths
    #[rstest]
    fn test_complete_collection_path(_current_dir: CurrentDirGuard) {
        let completions = complete(complete_collection_path());
        assert_eq!(&completions, &["other.yml", "slumber.yml"]);
    }

    /// Complete collection IDs from the DB and YAML file paths
    #[rstest]
    fn test_complete_collection_specifier(
        _current_dir: CurrentDirGuard,
        database: TestDatabase,
    ) {
        // Add a collection to the DB
        let collection_db = database
            .database
            .into_collection(&collection_file())
            .unwrap();

        let completions = complete(complete_collection_specifier());
        assert_eq!(
            &completions,
            &[
                "other.yml",
                "slumber.yml",
                &collection_db.collection_id().to_string()
            ]
        );
    }

    /// Test prefix filtering on candidates
    #[test]
    fn test_get_candidates() {
        let candidates: Vec<String> = get_candidates(
            ["abc123", "abc", "bca"].into_iter(),
            OsStr::new("abc"),
        )
        .into_iter()
        .map(|candidate| candidate.get_value().to_str().unwrap().to_owned())
        .collect();
        assert_eq!(candidates, &["abc123", "abc"]);
    }

    fn complete(completer: ArgValueCompleter) -> Vec<String> {
        completer
            .complete(OsStr::new(""))
            .into_iter()
            .map(|completion| {
                completion.get_value().to_str().unwrap().to_owned()
            })
            .collect()
    }

    struct TestDatabase {
        database: Database,
        /// Hang onto this dir isn't deleted until the end of the test
        _data_dir: DataDir,
    }

    /// Create a DB in the temp dir. We don't want an in-memory DB because we
    /// want to test real world behavior. Returns the env guard as well because
    /// the env variable needs to remain set until the end of the test
    #[fixture]
    fn database(data_dir: DataDir) -> TestDatabase {
        let database = Database::load().unwrap();
        TestDatabase {
            database,
            _data_dir: data_dir,
        }
    }

    /// Get path to `crates/cli/tests/`
    fn tests_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests")
    }

    fn collection_file() -> CollectionFile {
        // Make sure to not make a call that checks env::current_dir(), because
        // that can be modified by other tests
        CollectionFile::with_dir(tests_dir(), None).unwrap()
    }

    /// Set the current directory to the directory containing test collection
    /// files. Return a guard that will reset the directory on drop. The cwd is
    /// global mutable state so this uses a mutex to prevent concurrent runs
    /// using the cwd.
    #[fixture]
    fn current_dir() -> CurrentDirGuard {
        env_lock::lock_current_dir(tests_dir()).unwrap()
    }
}
