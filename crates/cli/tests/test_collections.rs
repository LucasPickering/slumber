//! Test `slumber collections` command

mod common;

use crate::common::{collection_file, tests_dir};
use predicates::prelude::PredicateBooleanExt;
use rstest::rstest;
use slumber_core::{
    collection::{CollectionFile, RecipeId},
    database::Database,
    http::Exchange,
};
use slumber_util::Factory;
use std::{fs, path::Path};
use uuid::Uuid;

/// `slumber collections list`
#[rstest]
fn test_collections_list() {
    let (mut command, data_dir) = common::slumber();
    init_db(&data_dir);

    command
        .args(["collections", "list"])
        .assert()
        .success()
        .stdout(
            predicates::str::contains("slumber.yml")
                .and(predicates::str::contains("other.yml")),
        );
}

/// `slumber collections migrate` with paths as  arguments
///
/// The actual merge logic is tested in the database so we're just trying to
/// test the arg handling and basic functionality
#[test]
fn test_collections_migrate_paths() {
    let (mut command, data_dir) = common::slumber();
    let database = init_db(&data_dir);

    // Verify we start with 2 collections
    let collections = database.get_collections().unwrap();
    assert_eq!(collections.len(), 2);
    // Grab the first collection so we can ensure it's the only one left later.
    // Do a sanity check to make sure this is the one we're migrating TO
    let first_collection = &collections[0];
    assert!(
        first_collection.path.ends_with("slumber.yml"),
        "Expected target collection to be first in list"
    );

    // Merge the collections
    command
        .args(["collections", "migrate", "other.yml", "slumber.yml"])
        .assert()
        .success()
        .stdout("Migrated other.yml into slumber.yml\n");

    // 1 collection now, all the requests are under that collection
    let collections = database.get_collections().unwrap();
    assert_eq!(collections.len(), 1);
    assert_eq!(collections[0].id, first_collection.id);
    assert_eq!(database.get_all_requests().unwrap().len(), 3);
}

/// `slumber collections migrate` with IDs as arguments
#[test]
fn test_collections_migrate_ids() {
    let (mut command, data_dir) = common::slumber();
    let database = init_db(&data_dir);

    // Verify we start with 2 collections
    let collections = database.get_collections().unwrap();
    assert_eq!(collections.len(), 2);
    let id1 = collections[0].id;
    let id2 = collections[1].id;

    // Merge the collections
    command
        .args(["collections", "migrate", &id2.to_string(), &id1.to_string()])
        .assert()
        .success()
        .stdout(format!("Migrated {id2} into {id1}\n"));

    // 1 collection now, all the requests are under that collection
    let collections = database.get_collections().unwrap();
    assert_eq!(collections.len(), 1);
    assert_eq!(collections[0].id, id1);
    assert_eq!(database.get_all_requests().unwrap().len(), 3);
}

/// `slumber collections delete`
#[test]
fn test_collections_delete() {
    let (mut command, data_dir) = common::slumber();
    let database = init_db(&data_dir);

    // Verify we start with 2 collections and 3 requests
    let collections = database.get_collections().unwrap();
    assert_eq!(collections.len(), 2);
    let id = collections[0].id;
    assert_eq!(database.get_all_requests().unwrap().len(), 3);

    // Delete!!
    command
        .args(["collections", "delete", &id.to_string()])
        .assert()
        .success()
        .stdout(format!("Deleted collection {id}\n"));

    // 1 collection now. The requests of the deleted collection are gone
    let collections = database.get_collections().unwrap();
    assert_eq!(collections.len(), 1);
    assert_ne!(collections[0].id, id); // Deleted collection is NOT present
    assert_eq!(database.get_all_requests().unwrap().len(), 1);
}

/// Test collection deletion when the file is already gone. Should still work
#[test]
fn test_collections_delete_file_missing() {
    let (mut command, data_dir) = common::slumber();
    let database = Database::from_directory(&data_dir).unwrap();

    // Make a new collection file and add it to the DB. Pre-canonicalize it
    // because we won't be able to canonicalize after deletion. This is relevant
    // because some systems use symlinks in the tmp file system
    let collection_path = data_dir.join("slumber.yml");
    fs::write(&collection_path, "").unwrap();
    let collection_path = collection_path.canonicalize().unwrap();
    let collection_file =
        CollectionFile::new(Some(collection_path.clone())).unwrap();
    let id = database
        .clone()
        .into_collection(&collection_file)
        .unwrap()
        .collection_id();

    // Sanity check
    assert_eq!(
        database
            .get_collections()
            .unwrap()
            .into_iter()
            .map(|collection| collection.id)
            .collect::<Vec<_>>(),
        [id]
    );

    // Delete the file before deleting from the DB
    fs::remove_file(&collection_path).unwrap();
    command
        .args(["collections", "delete", collection_path.to_str().unwrap()])
        .assert()
        .success();

    assert_eq!(database.get_collections().unwrap(), []);
}

/// Passing an unknown ID to `slumber collections delete` gives an error
#[test]
fn test_collections_delete_bad_id() {
    let (mut command, _) = common::slumber();

    let id = Uuid::new_v4().to_string();
    command
        .args(["collections", "delete", &id])
        .assert()
        .failure()
        .stderr(format!("Unknown collection `{id}`\n"));
}

/// Initialize database with multiple collections and some exchanges
fn init_db(data_dir: &Path) -> Database {
    let database = Database::from_directory(data_dir).unwrap();

    let collection1_db = database
        .clone()
        .into_collection(&collection_file())
        .unwrap();
    collection1_db
        .insert_exchange(&Exchange::factory(RecipeId::from("getUser")))
        .unwrap();
    collection1_db
        .insert_exchange(&Exchange::factory(RecipeId::from("jsonBody")))
        .unwrap();

    let collection2_db = database
        .clone()
        .into_collection(
            &CollectionFile::new(Some(tests_dir().join("other.yml"))).unwrap(),
        )
        .unwrap();
    collection2_db
        .insert_exchange(&Exchange::factory(RecipeId::from("getUser")))
        .unwrap();

    database
}
