//! Test the `slumber history` subcommand

mod common;

use crate::common::collection_file;
use itertools::Itertools;
use rstest::rstest;
use slumber_core::{
    collection::{CollectionFile, ProfileId, RecipeId},
    database::Database,
    http::{Exchange, RequestId},
};
use slumber_util::{Factory, paths::get_repo_root};
use std::path::Path;
use uuid::Uuid;

// Use static IDs for the recipes so we can refer to them in expectations
const RECIPE1_NO_PROFILE_ID: RequestId =
    id("00000000-0000-0000-0000-000000000000");
const RECIPE1_PROFILE1_ID: RequestId =
    id("00000000-0000-0000-0000-000000000001");
const RECIPE2_ID: RequestId = id("00000000-0000-0000-0000-000000000002");
const OTHER_COLLECTION_ID: RequestId =
    id("00000000-0000-0000-0000-000000000003");

/// Test `slumber history list`
#[rstest]
#[case::recipe(
    &["history", "list", "recipe1"],
    &[RECIPE1_NO_PROFILE_ID, RECIPE1_PROFILE1_ID],
)]
#[case::no_profile(
    &["history", "list", "recipe1", "-p"], &[RECIPE1_NO_PROFILE_ID],
)]
#[case::profile(
    &["history", "list", "recipe1", "-p", "profile1"], &[RECIPE1_PROFILE1_ID],
)]
#[case::collection(
    &["history", "list"],
    &[RECIPE1_NO_PROFILE_ID, RECIPE1_PROFILE1_ID, RECIPE2_ID],
)]
#[case::different_collection(
    &["-f", "../../../slumber.yml", "history", "list"],
    &[OTHER_COLLECTION_ID],
)]
#[case::all(
    &["history", "list", "--all"],
    &[RECIPE1_NO_PROFILE_ID, RECIPE1_PROFILE1_ID, RECIPE2_ID, OTHER_COLLECTION_ID],
)]
fn test_history_list(
    #[case] arguments: &[&str],
    #[case] expected_requests: &[RequestId],
) {
    let (mut command, data_dir) = common::slumber();
    init_db(&data_dir);

    command.args(arguments).assert().success().stdout(
        predicates::function::function(|stdout: &str| {
            expected_requests
                .iter()
                .all(|expected_id| stdout.contains(&expected_id.to_string()))
        }),
    );
}

/// Test `slumber history delete`
#[rstest]
fn test_history_delete() {
    let (mut command, data_dir) = common::slumber();
    let database = init_db(&data_dir);

    command
        .args([
            "history",
            "delete",
            &RECIPE1_PROFILE1_ID.to_string(),
            &RECIPE1_NO_PROFILE_ID.to_string(),
        ])
        .assert()
        .success();
    let remaining = database
        .get_all_requests()
        .unwrap()
        .into_iter()
        .map(|exchange| exchange.id)
        .sorted()
        .collect_vec();
    assert_eq!(&remaining, &[RECIPE2_ID, OTHER_COLLECTION_ID]);
}

const fn id(s: &str) -> RequestId {
    let Ok(uuid) = Uuid::try_parse(s) else {
        panic!("Bad value") // unwrap() isn't const
    };
    RequestId(uuid)
}

fn init_db(data_dir: &Path) -> Database {
    let database = Database::from_directory(data_dir).unwrap();
    let db = database
        .clone()
        .into_collection(&collection_file())
        .unwrap();
    let profile_id: ProfileId = "profile1".into();
    let recipe_id: RecipeId = "recipe1".into();
    db.insert_exchange(&Exchange::factory((
        RECIPE1_NO_PROFILE_ID,
        None,
        recipe_id.clone(),
    )))
    .unwrap();
    db.insert_exchange(&Exchange::factory((
        RECIPE1_PROFILE1_ID,
        Some(profile_id),
        recipe_id,
    )))
    .unwrap();
    db.insert_exchange(&Exchange::factory((
        RECIPE2_ID,
        None,
        "recipe2".into(),
    )))
    .unwrap();

    // Add one under a different collection
    let db = database
        .clone()
        .into_collection(
            &CollectionFile::new(Some(get_repo_root().join("slumber.yml")))
                .unwrap(),
        )
        .unwrap();
    db.insert_exchange(&Exchange::factory(OTHER_COLLECTION_ID))
        .unwrap();

    database
}
