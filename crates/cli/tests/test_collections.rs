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
use std::path::Path;
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
    let collections = database.collections().unwrap();
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
    let collections = database.collections().unwrap();
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
    let collections = database.collections().unwrap();
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
    let collections = database.collections().unwrap();
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
    let collections = database.collections().unwrap();
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
    let collections = database.collections().unwrap();
    assert_eq!(collections.len(), 1);
    assert_ne!(collections[0].id, id); // Deleted collection is NOT present
    assert_eq!(database.get_all_requests().unwrap().len(), 1);
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
