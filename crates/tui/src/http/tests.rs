use super::*;
use crate::test_util::{TestHarness, harness};
use anyhow::anyhow;
use chrono::Utc;
use rstest::rstest;
use slumber_core::http::{
    Exchange, RequestBuildError, RequestError, RequestRecord,
};
use slumber_util::{Factory, assert_matches};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::time;

#[rstest]
fn test_get() {
    let mut store = RequestStore::new(CollectionDatabase::factory(()));
    let exchange = Exchange::factory(());
    let id = exchange.id;
    store
        .requests
        .insert(exchange.id, RequestState::response(exchange));

    // This is a bit jank, but since we can't clone exchanges, the only way
    // to get the value back for comparison is to access the map directly
    assert_eq!(store.get(id), Some(store.requests.get(&id).unwrap()));
    assert_eq!(store.get(RequestId::new()), None);
}

/// building->loading->success
#[rstest]
#[tokio::test]
async fn test_life_cycle_success() {
    let mut store = RequestStore::new(CollectionDatabase::factory(()));
    let exchange = Exchange::factory(());
    let id = exchange.id;

    // Update for each state in the life cycle
    store.start(
        id,
        exchange.request.profile_id.clone(),
        exchange.request.recipe_id.clone(),
        Some(tokio::spawn(async {}).abort_handle()),
    );
    assert_matches!(store.get(id), Some(RequestState::Building { .. }));

    store.loading(Arc::clone(&exchange.request));
    assert_matches!(store.get(id), Some(RequestState::Loading { .. }));

    store.response(exchange);
    assert_matches!(store.get(id), Some(RequestState::Response { .. }));

    // Insert a new request, just to make sure it's independent
    let exchange2 = Exchange::factory(());
    let id2 = exchange2.id;
    store.start(
        id2,
        exchange2.request.profile_id.clone(),
        exchange2.request.recipe_id.clone(),
        Some(tokio::spawn(async {}).abort_handle()),
    );
    assert_matches!(store.get(id), Some(RequestState::Response { .. }));
    assert_matches!(store.get(id2), Some(RequestState::Building { .. }));
}

/// building->error
#[rstest]
#[tokio::test]
async fn test_life_cycle_build_error() {
    let mut store = RequestStore::new(CollectionDatabase::factory(()));
    let exchange = Exchange::factory(());
    let id = exchange.id;
    let profile_id = &exchange.request.profile_id;
    let recipe_id = &exchange.request.recipe_id;

    store.start(
        id,
        profile_id.clone(),
        recipe_id.clone(),
        Some(tokio::spawn(async {}).abort_handle()),
    );
    assert_matches!(store.get(id), Some(RequestState::Building { .. }));

    store.build_error(
        RequestBuildError {
            profile_id: profile_id.clone(),
            recipe_id: recipe_id.clone(),
            id,
            start_time: Utc::now(),
            end_time: Utc::now(),
            source: anyhow!("oh no!"),
        }
        .into(),
    );
    assert_matches!(store.get(id), Some(RequestState::BuildError { .. }));
}

/// building->loading->error
#[rstest]
#[tokio::test]
async fn test_life_cycle_request_error() {
    let mut store = RequestStore::new(CollectionDatabase::factory(()));
    let exchange = Exchange::factory(());
    let id = exchange.id;
    let profile_id = &exchange.request.profile_id;
    let recipe_id = &exchange.request.recipe_id;

    store.start(
        id,
        profile_id.clone(),
        recipe_id.clone(),
        Some(tokio::spawn(async {}).abort_handle()),
    );
    assert_matches!(store.get(id), Some(RequestState::Building { .. }));

    store.loading(Arc::clone(&exchange.request));
    assert_matches!(store.get(id), Some(RequestState::Loading { .. }));

    store.request_error(
        RequestError {
            error: anyhow!("oh no!"),
            request: exchange.request,
            start_time: Utc::now(),
            end_time: Utc::now(),
        }
        .into(),
    );
    assert_matches!(store.get(id), Some(RequestState::RequestError { .. }));
}

/// building->cancelled and loading->cancelled
#[rstest]
#[tokio::test]
async fn test_life_cycle_cancel() {
    let mut store = RequestStore::new(CollectionDatabase::factory(()));
    let exchange = Exchange::factory(());
    let id = exchange.id;
    let profile_id = &exchange.request.profile_id;
    let recipe_id = &exchange.request.recipe_id;

    // This flag confirms that neither future ever finishes
    let future_finished: Arc<AtomicBool> = Default::default();

    let ff = Arc::clone(&future_finished);
    store.start(
        id,
        profile_id.clone(),
        recipe_id.clone(),
        Some(
            tokio::spawn(async move {
                time::sleep(Duration::from_secs(1)).await;
                ff.store(true, Ordering::Relaxed);
            })
            .abort_handle(),
        ),
    );
    store.cancel(id);
    assert_matches!(store.get(id), Some(RequestState::Cancelled { .. }));
    assert!(!future_finished.load(Ordering::Relaxed));

    let ff = Arc::clone(&future_finished);
    store.start(
        id,
        profile_id.clone(),
        recipe_id.clone(),
        Some(
            tokio::spawn(async move {
                time::sleep(Duration::from_secs(1)).await;
                ff.store(true, Ordering::Relaxed);
            })
            .abort_handle(),
        ),
    );
    store.loading(exchange.request);
    assert_matches!(store.get(id), Some(RequestState::Loading { .. }));
    store.cancel(id);
    assert_matches!(store.get(id), Some(RequestState::Cancelled { .. }));
    assert!(!future_finished.load(Ordering::Relaxed));
}

#[rstest]
fn test_load(harness: TestHarness) {
    let mut store = harness.request_store.borrow_mut();

    // Generally we would expect this to be in the DB, but in this case omit
    // it so we can ensure the store *isn't* going to the DB for it
    let present_exchange = Exchange::factory(());
    let present_id = present_exchange.id;
    store
        .requests
        .insert(present_id, RequestState::response(present_exchange));

    let missing_exchange = Exchange::factory(());
    let missing_id = missing_exchange.id;
    harness.database.insert_exchange(&missing_exchange).unwrap();

    // Already in store, don't fetch
    assert_matches!(store.get(present_id), Some(RequestState::Response { .. }));
    assert_matches!(
        store.load(present_id),
        Ok(Some(RequestState::Response { .. }))
    );
    assert_matches!(store.get(present_id), Some(RequestState::Response { .. }));

    // Not in store, fetch successfully
    assert!(store.get(missing_id).is_none());
    assert_matches!(
        store.load(missing_id),
        Ok(Some(RequestState::Response { .. }))
    );
    assert_matches!(store.get(missing_id), Some(RequestState::Response { .. }));

    // Not in store and not in DB, return None
    assert_matches!(store.load(RequestId::new()), Ok(None));
}

#[rstest]
fn test_load_latest(harness: TestHarness) {
    let mut store = harness.request_store.borrow_mut();
    let profile_id = ProfileId::factory(());
    let recipe_id = RecipeId::factory(());

    // Create some confounding exchanges, that we don't expected to load
    create_exchange(&harness, Some(&profile_id), Some(&recipe_id));
    create_exchange(&harness, Some(&profile_id), None);
    create_exchange(&harness, None, Some(&recipe_id));
    let expected_exchange =
        create_exchange(&harness, Some(&profile_id), Some(&recipe_id));

    assert_eq!(
        store.load_latest((&profile_id).into(), &recipe_id).unwrap(),
        Some(&RequestState::response(expected_exchange))
    );

    // Non-match
    assert_matches!(
        store.load_latest((&profile_id).into(), &("other".into())),
        Ok(None)
    );
}

/// Test load_latest when the most recent request for the profile is a
/// request that's not in the DB (i.e. in a state other than completed)
#[rstest]
fn test_load_latest_local(harness: TestHarness) {
    let profile_id = ProfileId::factory(());
    let recipe_id = RecipeId::factory(());

    // We don't expect to load this one
    create_exchange(&harness, Some(&profile_id), Some(&recipe_id));

    // This is what we should see
    let exchange =
        Exchange::factory((Some(profile_id.clone()), recipe_id.clone()));
    let request_id = exchange.id;

    let mut store = harness.request_store.borrow_mut();
    store
        .requests
        .insert(exchange.id, RequestState::response(exchange));
    let loaded = store.load_latest((&profile_id).into(), &recipe_id).unwrap();
    assert_eq!(loaded.map(RequestState::id), Some(request_id));
}

#[rstest]
#[tokio::test]
async fn test_load_summaries(harness: TestHarness) {
    let mut store = harness.request_store.borrow_mut();
    let profile_id = ProfileId::factory(());
    let recipe_id = RecipeId::factory(());

    let mut exchanges = (0..5)
        .map(|_| create_exchange(&harness, Some(&profile_id), Some(&recipe_id)))
        .collect_vec();
    // Create some confounders
    create_exchange(&harness, None, Some(&recipe_id));
    create_exchange(&harness, Some(&profile_id), None);

    // Add one request of each possible state. We expect to get em all back
    // Pre-load one from the DB, to make sure it gets de-duped
    let exchange = exchanges.pop().unwrap();
    let response_id = exchange.id;
    store
        .requests
        .insert(exchange.id, RequestState::response(exchange));

    let building_id = RequestId::new();
    store.start(
        building_id,
        Some(profile_id.clone()),
        recipe_id.clone(),
        Some(tokio::spawn(async {}).abort_handle()),
    );

    let build_error_id = RequestId::new();
    store.requests.insert(
        build_error_id,
        RequestState::BuildError {
            error: RequestBuildError {
                profile_id: Some(profile_id.clone()),
                recipe_id: recipe_id.clone(),
                id: build_error_id,
                start_time: Utc::now(),
                end_time: Utc::now(),
                source: anyhow!("oh no!"),
            }
            .into(),
        },
    );

    let request =
        RequestRecord::factory((Some(profile_id.clone()), recipe_id.clone()));
    let loading_id = request.id;
    store.requests.insert(
        loading_id,
        RequestState::Loading {
            request: request.into(),
            start_time: Utc::now(),
            abort_handle: Some(tokio::spawn(async {}).abort_handle()),
        },
    );

    let request =
        RequestRecord::factory((Some(profile_id.clone()), recipe_id.clone()));
    let request_error_id = request.id;
    store.requests.insert(
        request_error_id,
        RequestState::RequestError {
            error: RequestError {
                error: anyhow!("oh no!"),
                request: request.into(),
                start_time: Utc::now(),
                end_time: Utc::now(),
            }
            .into(),
        },
    );

    // Neither of these should appear
    store.start(
        RequestId::new(),
        Some(ProfileId::factory(())),
        recipe_id.clone(),
        Some(tokio::spawn(async {}).abort_handle()),
    );
    store.start(
        RequestId::new(),
        Some(profile_id.clone()),
        RecipeId::factory(()),
        Some(tokio::spawn(async {}).abort_handle()),
    );

    // It's really annoying to do a full equality comparison because we'd
    // have to re-create each piece of data (they don't impl Clone), so
    // instead do a pattern match, then check the IDs
    let loaded = store
        .load_summaries(Some(&profile_id), &recipe_id)
        .unwrap()
        .collect_vec();
    assert_matches!(
        loaded.as_slice(),
        &[
            RequestStateSummary::RequestError { .. },
            RequestStateSummary::Loading { .. },
            RequestStateSummary::BuildError { .. },
            RequestStateSummary::Building { .. },
            RequestStateSummary::Response { .. },
            RequestStateSummary::Response { .. },
            RequestStateSummary::Response { .. },
            RequestStateSummary::Response { .. },
            RequestStateSummary::Response { .. },
        ]
    );

    let ids = loaded.iter().map(RequestStateSummary::id).collect_vec();
    // These should be sorted descending by start time, with dupes removed
    assert_eq!(
        ids.as_slice(),
        &[
            request_error_id,
            loading_id,
            build_error_id,
            building_id,
            response_id, // This one got de-duped
            exchanges[3].id,
            exchanges[2].id,
            exchanges[1].id,
            exchanges[0].id,
        ]
    );
}

/// Test deleting all requests for a recipe. This tests a single profile filter
/// as well as all profiles
#[rstest]
fn test_delete_recipe_requests(harness: TestHarness) {
    let recipe1 = RecipeId::factory(());
    let recipe2 = RecipeId::factory(());
    let profile1 = ProfileId::factory(());
    let profile2 = ProfileId::factory(());
    let r1p1_id = create_exchange(&harness, Some(&profile1), Some(&recipe1)).id;
    let r1p2_id = create_exchange(&harness, Some(&profile2), Some(&recipe1)).id;
    let r2p1_id = create_exchange(&harness, Some(&profile1), Some(&recipe2)).id;
    let r2p2_id = create_exchange(&harness, Some(&profile2), Some(&recipe2)).id;
    let all_ids = [r1p1_id, r2p1_id, r1p2_id, r2p2_id];

    let mut store = harness.request_store.borrow_mut();

    // Load everything into the cache. We'll do this after each modification to
    // make sure we're deleting from the cache AND the DB
    let load_all = |store: &mut RequestStore| {
        for id in all_ids {
            store.load(id).unwrap();
        }
    };

    let assert_present =
        |store: &mut RequestStore, expected_present: &[RequestId]| {
            // Assert that all the expected requests are present in both the
            // cache *and* the DB
            for id in all_ids {
                let cached = store.get(id);
                let db = harness.database.get_request(id).unwrap();
                if expected_present.contains(&id) {
                    assert!(
                        cached.is_some(),
                        "Expected {id} to be present in cache"
                    );
                    assert!(db.is_some(), "Expected {id} to be present in DB");
                } else {
                    assert_eq!(
                        cached, None,
                        "Expected {id} to be deleted from cache"
                    );
                    assert_eq!(db, None, "Expected {id} to be deleted from DB");
                }
            }
        };

    // Sanity check
    load_all(&mut store);
    assert_present(&mut store, &all_ids);

    // This delete should do nothing because there are no profile-less requests
    store
        .delete_recipe_requests(ProfileFilter::None, &recipe1)
        .unwrap();
    assert_present(&mut store, &all_ids);

    // Delete just p1/r1
    store
        .delete_recipe_requests(Some(&profile1).into(), &recipe1)
        .unwrap();
    assert_present(&mut store, &[r2p1_id, r1p2_id, r2p2_id]);

    // Delete all for r1
    store
        .delete_recipe_requests(ProfileFilter::All, &recipe1)
        .unwrap();
    assert_present(&mut store, &[r2p1_id, r2p2_id]);
}

/// Test deleting a single request
#[rstest]
fn test_delete_request(harness: TestHarness) {
    let id = create_exchange(&harness, None, None).id;

    // Load the exchange into the cache
    let mut store = harness.request_store.borrow_mut();
    assert!(store.load(id).unwrap().is_some());

    store.delete_request(id).unwrap();

    // It's gone
    assert_eq!(store.get(id), None);
    assert_eq!(harness.database.get_request(id).unwrap(), None);
}

/// Create a exchange with the given profile+recipe ID (or random if
/// None), and insert it into the DB
fn create_exchange(
    harness: &TestHarness,
    profile_id: Option<&ProfileId>,
    recipe_id: Option<&RecipeId>,
) -> Exchange {
    let exchange = Exchange::factory((
        Some(
            profile_id
                .cloned()
                .unwrap_or_else(|| ProfileId::factory(())),
        ),
        recipe_id.cloned().unwrap_or_else(|| RecipeId::factory(())),
    ));
    harness.database.insert_exchange(&exchange).unwrap();
    exchange
}
