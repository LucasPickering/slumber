use crate::{
    collection::{ProfileId, RecipeId},
    http::RequestId,
    tui::view::{
        context::ViewContext, state::RequestStateSummary, RequestState,
    },
};
use itertools::Itertools;
use std::collections::{hash_map::Entry, HashMap};

/// Simple in-memory "database" for request state. This serves a few purposes:
///
/// - Save all incomplete requests (in-progress or failed) from the current app
///   session. These do *not* get persisted in the database
/// - Cache historical requests from the database. If we're accessing them
///   repeatedly, we don't want to keep going back to the DB.
/// - Provide a simple unified interface over both the in-memory cache and the
///   persistent DB, so callers can simply ask for requests and we only go to
///   the DB when necessary.
///
/// These operations are generally fallible only when the underlying DB
/// operation fails.
#[derive(Debug, Default)]
pub struct RequestStore {
    requests: HashMap<RequestId, RequestState>,
}

impl RequestStore {
    /// Get request state by ID
    pub fn get(&self, id: RequestId) -> Option<&RequestState> {
        self.requests.get(&id)
    }

    /// Update state of an in-progress HTTP request. Return `true` if the
    /// request is **new** in the state, i.e. it's the initial insert
    pub fn update(&mut self, state: RequestState) -> bool {
        self.requests.insert(state.id(), state).is_none()
    }

    /// Load a request from the database by ID. If already present in the store,
    /// do *not* update it. Only go to the DB if it's missing. Return the loaded
    /// request. Return `None` only if the ID is not present in the store *or*
    /// the DB.
    pub fn load(
        &mut self,
        id: RequestId,
    ) -> anyhow::Result<Option<&RequestState>> {
        let request = match self.requests.entry(id) {
            Entry::Occupied(entry) => Some(entry.into_mut()),
            Entry::Vacant(entry) => {
                ViewContext::with_database(|database| database.get_request(id))?
                    .map(|exchange| {
                        entry.insert(RequestState::response(exchange))
                    })
            }
        };
        Ok(request.map(|r| &*r))
    }

    /// Get the latest request for a specific profile+recipe combo
    pub fn load_latest(
        &mut self,
        profile_id: Option<&ProfileId>,
        recipe_id: &RecipeId,
    ) -> anyhow::Result<Option<&RequestState>> {
        let exchange = ViewContext::with_database(|database| {
            database.get_latest_request(profile_id, recipe_id)
        })?;
        let state = exchange.map(|exchange| {
            let state = RequestState::response(exchange);
            // Insert into the map, get a reference back
            // unstable: https://doc.rust-lang.org/std/collections/hash_map/enum.Entry.html#method.insert_entry
            match self.requests.entry(state.id()) {
                Entry::Occupied(mut entry) => {
                    entry.insert(state);
                    entry.into_mut() as &_ // Drop mutability
                }
                Entry::Vacant(entry) => entry.insert(state),
            }
        });
        Ok(state)
    }

    /// Load all historical requests for a recipe+profile, then return the
    /// *entire* set of requests, including in-progress ones. Returned requests
    /// are just summaries, not the full request. This is intended for list
    /// views, so we don't need to load the entire request/response for each
    /// one. Results are sorted by request *start* time, descending.
    pub fn load_summaries<'a>(
        &'a self,
        profile_id: Option<&'a ProfileId>,
        recipe_id: &'a RecipeId,
    ) -> anyhow::Result<impl 'a + Iterator<Item = RequestStateSummary>> {
        // Load summaries from the DB. We do *not* want to insert these into the
        // store, because they don't include request/response data
        let loaded = ViewContext::with_database(|database| {
            database.get_all_requests(profile_id, recipe_id)
        })?;

        // Find what we have in memory already
        let iter = self
            .requests
            .values()
            .filter(move |state| {
                state.profile_id() == profile_id
                    && state.recipe_id() == recipe_id
            })
            .map(RequestStateSummary::from)
            // Add what we loaded from the DB
            .chain(loaded.into_iter().map(RequestStateSummary::Response))
            // Sort descending
            .sorted_by_key(RequestStateSummary::time)
            .rev()
            // De-duplicate double-loaded requests
            .unique_by(RequestStateSummary::id);
        Ok(iter)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        http::{Exchange, RequestBuildError, RequestError, RequestRecord},
        test_util::{assert_matches, Factory},
        tui::test_util::{harness, TestHarness},
    };
    use anyhow::anyhow;
    use chrono::Utc;
    use rstest::rstest;
    use std::sync::Arc;

    #[test]
    fn test_get() {
        let exchange = Exchange::factory(());
        let id = exchange.id;
        let mut store = RequestStore::default();
        store
            .requests
            .insert(exchange.id, RequestState::response(exchange));

        // This is a bit jank, but since we can't clone exchanges, the only way
        // to get the value back for comparison is to access the map directly
        assert_eq!(store.get(id), Some(store.requests.get(&id).unwrap()));
        assert_eq!(store.get(RequestId::new()), None);
    }

    #[test]
    fn test_update() {
        let exchange = Exchange::factory(());
        let id = exchange.id;
        let mut store = RequestStore::default();

        // Update for each state in the life cycle
        assert!(store.update(RequestState::Building {
            id,
            start_time: exchange.start_time,
            profile_id: exchange.request.profile_id.clone(),
            recipe_id: exchange.request.recipe_id.clone()
        }));
        assert_matches!(store.get(id), Some(RequestState::Building { .. }));

        assert!(!store.update(RequestState::Loading {
            request: Arc::clone(&exchange.request),
            start_time: exchange.start_time,
        }));
        assert_matches!(store.get(id), Some(RequestState::Loading { .. }));

        assert!(!store.update(RequestState::response(exchange)));
        assert_matches!(store.get(id), Some(RequestState::Response { .. }));

        // Insert a new request, just to make sure it's independent
        let exchange2 = Exchange::factory(());
        let id2 = exchange2.id;
        assert!(store.update(RequestState::Building {
            id: id2,
            start_time: exchange2.start_time,
            profile_id: exchange2.request.profile_id.clone(),
            recipe_id: exchange2.request.recipe_id.clone()
        }));
        assert_matches!(store.get(id), Some(RequestState::Response { .. }));
        assert_matches!(store.get(id2), Some(RequestState::Building { .. }));
    }

    #[rstest]
    fn test_load(harness: TestHarness) {
        let mut store = RequestStore::default();

        // Generally we would expect this to be in the DB, but in this case omit
        // it so we can ensure the store *isn't* going to the DB for it
        let present_exchange = Exchange::factory(());
        let present_id = present_exchange.id;

        let missing_exchange = Exchange::factory(());
        let missing_id = missing_exchange.id;
        harness.database.insert_exchange(&missing_exchange).unwrap();

        // Already in store, don't fetch
        store
            .requests
            .insert(present_id, RequestState::response(present_exchange));
        assert_matches!(
            store.get(present_id),
            Some(RequestState::Response { .. })
        );
        assert_matches!(
            store.load(present_id),
            Ok(Some(RequestState::Response { .. }))
        );
        assert_matches!(
            store.get(present_id),
            Some(RequestState::Response { .. })
        );

        // Not in store, fetch successfully
        assert!(store.get(missing_id).is_none());
        assert_matches!(
            store.load(missing_id),
            Ok(Some(RequestState::Response { .. }))
        );
        assert_matches!(
            store.get(missing_id),
            Some(RequestState::Response { .. })
        );

        // Not in store and not in DB, return None
        assert_matches!(store.load(RequestId::new()), Ok(None));
    }

    #[rstest]
    fn test_load_latest(harness: TestHarness) {
        let profile_id = ProfileId::factory(());
        let recipe_id = RecipeId::factory(());

        // Create some confounding exchanges, that we don't expected to load
        create_exchange(&harness, Some(&profile_id), Some(&recipe_id));
        create_exchange(&harness, Some(&profile_id), None);
        create_exchange(&harness, None, Some(&recipe_id));
        let expected_exchange =
            create_exchange(&harness, Some(&profile_id), Some(&recipe_id));

        let mut store = RequestStore::default();
        assert_eq!(
            store.load_latest(Some(&profile_id), &recipe_id).unwrap(),
            Some(&RequestState::response(expected_exchange))
        );

        // Non-match
        assert_matches!(
            store.load_latest(Some(&profile_id), &("other".into())),
            Ok(None)
        );
    }

    #[rstest]
    fn test_load_summaries(harness: TestHarness) {
        let profile_id = ProfileId::factory(());
        let recipe_id = RecipeId::factory(());

        let mut exchanges = (0..5)
            .map(|_| {
                create_exchange(&harness, Some(&profile_id), Some(&recipe_id))
            })
            .collect_vec();
        // Create some confounders
        create_exchange(&harness, None, Some(&recipe_id));
        create_exchange(&harness, Some(&profile_id), None);

        // Add one request of each possible state. We expect to get em all back
        let mut store = RequestStore::default();

        // Pre-load one from the DB, to make sure it gets de-duped
        let exchange = exchanges.pop().unwrap();
        let response_id = exchange.id;
        store.update(RequestState::response(exchange));

        let building_id = RequestId::new();
        store.update(RequestState::Building {
            id: building_id,
            start_time: Utc::now(),
            profile_id: Some(profile_id.clone()),
            recipe_id: recipe_id.clone(),
        });

        let build_error_id = RequestId::new();
        store.update(RequestState::BuildError {
            error: RequestBuildError {
                profile_id: Some(profile_id.clone()),
                recipe_id: recipe_id.clone(),
                id: build_error_id,
                time: Utc::now(),
                error: anyhow!("oh no!"),
            },
        });

        let request = RequestRecord::factory((
            Some(profile_id.clone()),
            recipe_id.clone(),
        ));
        let loading_id = request.id;
        store.update(RequestState::Loading {
            request: request.into(),
            start_time: Utc::now(),
        });

        let request = RequestRecord::factory((
            Some(profile_id.clone()),
            recipe_id.clone(),
        ));
        let request_error_id = request.id;
        store.update(RequestState::RequestError {
            error: RequestError {
                error: anyhow!("oh no!"),
                request: request.into(),
                start_time: Utc::now(),
                end_time: Utc::now(),
            },
        });

        // Neither of these should appear
        store.update(RequestState::Building {
            id: RequestId::new(),
            start_time: Utc::now(),
            profile_id: Some(ProfileId::factory(())),
            recipe_id: recipe_id.clone(),
        });
        store.update(RequestState::Building {
            id: RequestId::new(),
            start_time: Utc::now(),
            profile_id: Some(profile_id.clone()),
            recipe_id: RecipeId::factory(()),
        });

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
}
