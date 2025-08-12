use super::*;
use indexmap::IndexMap;
use itertools::Itertools;
use rstest::{fixture, rstest};
use slumber_util::{Factory, paths::get_repo_root};
use std::collections::HashMap;

impl CollectionDatabase {
    fn count_requests(&self) -> usize {
        self.database
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM requests_v2
                WHERE collection_id = :collection_id",
                named_params! {
                    ":collection_id": self.collection_id(),
                },
                |row| row.get(0),
            )
            .unwrap()
    }
}

#[fixture]
fn collection_file() -> CollectionFile {
    CollectionFile::new(Some(get_repo_root().join("slumber.yml"))).unwrap()
}

#[fixture]
fn other_collection_file() -> CollectionFile {
    // Has to be a real file
    CollectionFile::new(Some(get_repo_root().join("README.md"))).unwrap()
}

/// Populate a DB with two collections, and a few exchanges in each one
#[fixture]
fn request_db(
    collection_file: CollectionFile,
    other_collection_file: CollectionFile,
) -> RequestDb {
    let database = Database::factory(());
    let collection1 =
        database.clone().into_collection(&collection_file).unwrap();
    let collection2 = database
        .clone()
        .into_collection(&other_collection_file)
        .unwrap();

    // We separate requests by 3 columns. Create multiple of each column to
    // make sure we filter by each column correctly
    let collections = [collection1, collection2];

    // Store the created request ID for each cell in the matrix, so we can
    // compare to what the DB spits back later
    let mut request_ids: IndexMap<
        (CollectionId, Option<ProfileId>, RecipeId),
        RequestId,
    > = Default::default();

    // Create and insert each request
    for collection in &collections {
        for profile_id in [None, Some("profile1"), Some("profile2")] {
            for recipe_id in ["recipe1", "recipe2"] {
                let recipe_id: RecipeId = recipe_id.into();
                let profile_id = profile_id.map(ProfileId::from);
                let exchange =
                    Exchange::factory((profile_id.clone(), recipe_id.clone()));
                collection.insert_exchange(&exchange).unwrap();
                request_ids.insert(
                    (collection.collection_id(), profile_id, recipe_id),
                    exchange.id,
                );
            }
        }
    }

    RequestDb {
        database,
        collections,
        request_ids,
    }
}

struct RequestDb {
    database: Database,
    collections: [CollectionDatabase; 2],
    /// A map of the request IDs we inserted for each (collection, profile,
    /// recipe) key. This makes it possible to do assertions on the inserted
    /// IDs
    request_ids:
        IndexMap<(CollectionId, Option<ProfileId>, RecipeId), RequestId>,
}

#[rstest]
fn test_collection_delete(collection_file: CollectionFile) {
    let database = Database::factory(());
    let collection =
        database.clone().into_collection(&collection_file).unwrap();

    let exchange = Exchange::factory(RecipeId::from("recipe1"));
    let key_type = "MyKey";
    let ui_key = "key1";
    collection.insert_exchange(&exchange).unwrap();
    collection.set_ui(key_type, ui_key, "value1").unwrap();

    // Sanity checks
    assert_eq!(collection.get_all_requests().unwrap().len(), 1);
    assert_eq!(
        collection.get_ui::<_, String>(key_type, ui_key).unwrap(),
        Some("value1".into())
    );

    // Do the delete
    database
        .delete_collection(collection.collection_id)
        .unwrap();

    // All gone!
    assert_eq!(database.get_collections().unwrap(), []);
    assert_eq!(database.get_all_requests().unwrap(), []);
    assert_eq!(
        collection.get_ui::<_, String>(key_type, ui_key).unwrap(),
        None
    );
}

#[rstest]
fn test_collection_merge(
    collection_file: CollectionFile,
    other_collection_file: CollectionFile,
) {
    let database = Database::factory(());
    let collection1 =
        database.clone().into_collection(&collection_file).unwrap();
    let collection2 = database
        .clone()
        .into_collection(&other_collection_file)
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
            .get_latest_request(profile_id.into(), recipe_id)
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
            .get_latest_request(profile_id.into(), recipe_id)
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
    database
        .merge_collections(collection2.collection_id, collection1.collection_id)
        .unwrap();

    // Collection 2 values should've overwritten
    assert_eq!(
        collection1
            .get_latest_request(profile_id.into(), recipe_id)
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
        database
            .get_collections()
            .unwrap()
            .into_iter()
            .map(|collection| collection.path)
            .collect::<Vec<_>>(),
        vec![collection_file.path().canonicalize().unwrap()]
    );
}

/// Test fetching all requests for the whole DB
#[rstest]
fn test_database_get_all_requests(request_db: RequestDb) {
    assert_eq!(request_db.database.get_all_requests().unwrap().len(), 12);
}

/// Test getting most recent request by recipe/profile
#[rstest]
fn test_get_latest_request(request_db: RequestDb) {
    // Try to find each inserted recipe individually. Also try some
    // expected non-matches
    for collection in &request_db.collections {
        for profile_id in [None, Some("profile1"), Some("extra_profile")] {
            for recipe_id in ["recipe1", "extra_recipe"] {
                let collection_id = collection.collection_id();
                let profile_id = profile_id.map(ProfileId::from);
                let recipe_id = recipe_id.into();

                // Leave the Option here so a non-match will trigger a handy
                // assertion error
                let exchange_id = collection
                    .get_latest_request(profile_id.as_ref().into(), &recipe_id)
                    .unwrap()
                    .map(|exchange| exchange.id);
                let expected_id = request_db.request_ids.get(&(
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

/// Test fetching all requests for a collection
#[rstest]
fn test_collection_get_all_requests(request_db: RequestDb) {
    let [collection1, collection2] = request_db.collections;
    assert_eq!(collection1.get_all_requests().unwrap().len(), 6);
    assert_eq!(collection2.get_all_requests().unwrap().len(), 6);
}

/// Test fetching all requests for a single recipe
#[test]
fn test_get_recipe_requests() {
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
                .get_recipe_requests(profile_id.as_ref().into(), &recipe_id)
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
        .get_recipe_requests(ProfileFilter::All, &recipe_id)
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
    assert_eq!(ids, expected_ids);
}

/// Test deleting all requests for a recipe/profile combo
#[rstest]
#[case(ProfileFilter::All, "recipe1", 3)]
#[case(ProfileFilter::Some(Cow::Owned("profile1".into())), "recipe1", 1)]
#[case(ProfileFilter::None, "recipe1", 1)]
fn test_delete_recipe_requests(
    request_db: RequestDb,
    #[case] profile_filter: ProfileFilter<'static>,
    #[case] recipe_id: RecipeId,
    #[case] expected_deleted: usize,
) {
    let [collection1, collection2] = request_db.collections;
    assert_eq!(
        collection1
            .delete_recipe_requests(profile_filter, &recipe_id)
            .unwrap(),
        expected_deleted
    );
    assert_eq!(collection1.count_requests(), 6 - expected_deleted);
    assert_eq!(collection2.count_requests(), 6);
}

/// Test deleting a specific request
#[rstest]
fn test_delete_request(request_db: RequestDb) {
    let [collection1, collection2] = request_db.collections;
    let request_id = *request_db.request_ids.first().unwrap().1;

    assert_eq!(request_db.database.delete_request(request_id).unwrap(), 1);
    assert_eq!(collection1.count_requests(), 5);
    assert_eq!(collection2.count_requests(), 6);
}

/// Test UI state storage and retrieval
#[rstest]
fn test_ui_state(
    collection_file: CollectionFile,
    other_collection_file: CollectionFile,
) {
    let database = Database::factory(());
    let collection1 =
        database.clone().into_collection(&collection_file).unwrap();
    let collection2 = database
        .clone()
        .into_collection(&other_collection_file)
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
