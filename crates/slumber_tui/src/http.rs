//! Types for managing HTTP state in the TUI

use chrono::{DateTime, Duration, Utc};
use itertools::Itertools;
use reqwest::StatusCode;
use slumber_core::{
    collection::{ProfileId, RecipeId},
    db::CollectionDatabase,
    http::{
        Exchange, ExchangeSummary, RequestBuildError, RequestError, RequestId,
        RequestRecord,
    },
};
use std::{
    collections::{hash_map::Entry, HashMap},
    sync::Arc,
};

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
#[derive(Debug)]
pub struct RequestStore {
    database: CollectionDatabase,
    requests: HashMap<RequestId, RequestState>,
}

impl RequestStore {
    pub fn new(database: CollectionDatabase) -> Self {
        Self {
            database,
            requests: Default::default(),
        }
    }

    /// Are any requests in flight?
    pub fn has_active_requests(&self) -> bool {
        self.requests
            .values()
            .any(|state| matches!(state, RequestState::Loading { .. }))
    }

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
            Entry::Vacant(entry) => self
                .database
                .get_request(id)?
                .map(|exchange| entry.insert(RequestState::response(exchange))),
        };
        Ok(request.map(|r| &*r))
    }

    /// Get the latest request (by start time) for a specific profile+recipe
    /// combo
    pub fn load_latest(
        &mut self,
        profile_id: Option<&ProfileId>,
        recipe_id: &RecipeId,
    ) -> anyhow::Result<Option<&RequestState>> {
        // Get the latest record in the DB
        let exchange =
            self.database.get_latest_request(profile_id, recipe_id)?;
        if let Some(exchange) = exchange {
            // Cache this record if it isn't already
            self.requests
                .entry(exchange.id)
                // This is expensive because it parses the body, so avoid it if
                // the record is already cached
                .or_insert_with(|| RequestState::response(exchange));
        }

        // Now that the know the most recent completed record is in our local
        // cache, find the most recent record of *any* kind

        Ok(self
            .requests
            .values()
            .filter(|state| {
                state.profile_id() == profile_id
                    && state.recipe_id() == recipe_id
            })
            .max_by_key(|state| state.request_metadata().start_time))
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
        let loaded = self.database.get_all_requests(profile_id, recipe_id)?;

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

/// State of an HTTP response, which can be in various states of
/// completion/failure. Each request *recipe* should have one request state
/// stored in the view at a time.
#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq))]
pub enum RequestState {
    /// The request is being built. Typically this is very fast, but can be
    /// slow if a chain source takes a while.
    Building {
        id: RequestId,
        start_time: DateTime<Utc>,
        profile_id: Option<ProfileId>,
        recipe_id: RecipeId,
    },

    /// Something went wrong during the build :(
    BuildError { error: RequestBuildError },

    /// Request is in flight, or is *about* to be sent. There's no way to
    /// initiate a request that doesn't immediately launch it, so Loading is
    /// the initial state.
    Loading {
        /// This needs an Arc so the success/failure state can maintain a
        /// pointer to the request as well
        request: Arc<RequestRecord>,
        start_time: DateTime<Utc>,
    },

    /// A resolved HTTP response, with all content loaded and ready to be
    /// displayed. This does *not necessarily* have a 2xx/3xx status code, any
    /// received response is considered a "success".
    Response { exchange: Exchange },

    /// Error occurred sending the request or receiving the response.
    RequestError { error: RequestError },
}

/// Metadata derived from a request. The request can be in progress, completed,
/// or failed.
#[derive(Debug)]
pub struct RequestMetadata {
    /// When was the request launched?
    pub start_time: DateTime<Utc>,
    /// Elapsed time for the active request. If pending, this is a running
    /// total. Otherwise end time - start time.
    pub duration: Duration,
}

/// Metadata derived from a response. This is only available for requests that
/// have completed successfully.
#[derive(Debug)]
pub struct ResponseMetadata {
    pub status: StatusCode,
    /// Size of the response *body*
    pub size: usize,
}

impl RequestState {
    /// Unique ID for this request, which will be retained throughout its life
    /// cycle
    pub fn id(&self) -> RequestId {
        match self {
            Self::Building { id, .. } => *id,
            Self::BuildError { error, .. } => error.id,
            Self::Loading { request, .. } => request.id,
            Self::RequestError { error } => error.request.id,
            Self::Response { exchange, .. } => exchange.id,
        }
    }

    /// The profile that the request was rendered from
    pub fn profile_id(&self) -> Option<&ProfileId> {
        match self {
            Self::Building { profile_id, .. } => profile_id.as_ref(),
            Self::BuildError { error } => error.profile_id.as_ref(),
            Self::Loading { request, .. } => request.profile_id.as_ref(),
            Self::RequestError { error } => error.request.profile_id.as_ref(),
            Self::Response { exchange, .. } => {
                exchange.request.profile_id.as_ref()
            }
        }
    }

    /// The recipe that the request was rendered from
    pub fn recipe_id(&self) -> &RecipeId {
        match self {
            Self::Building { recipe_id, .. } => recipe_id,
            Self::BuildError { error } => &error.recipe_id,
            Self::Loading { request, .. } => &request.recipe_id,
            Self::RequestError { error } => &error.request.recipe_id,
            Self::Response { exchange, .. } => &exchange.request.recipe_id,
        }
    }

    /// Get metadata about a request. Return `None` if the request hasn't been
    /// successfully built (yet)
    pub fn request_metadata(&self) -> RequestMetadata {
        match self {
            // In-progress states
            Self::Building { start_time, .. }
            | Self::Loading { start_time, .. } => RequestMetadata {
                start_time: *start_time,
                duration: Utc::now() - start_time,
            },

            // Error states
            Self::BuildError {
                error:
                    RequestBuildError {
                        start_time,
                        end_time,
                        ..
                    },
            }
            | Self::RequestError {
                error:
                    RequestError {
                        start_time,
                        end_time,
                        ..
                    },
            } => RequestMetadata {
                start_time: *start_time,
                duration: *end_time - *start_time,
            },

            // Completed
            Self::Response { exchange, .. } => RequestMetadata {
                start_time: exchange.start_time,
                duration: exchange.duration(),
            },
        }
    }

    /// Get metadata about the request. Return `None` if the response hasn't
    /// been received, or the request failed.
    pub fn response_metadata(&self) -> Option<ResponseMetadata> {
        if let RequestState::Response { exchange } = self {
            Some(ResponseMetadata {
                status: exchange.response.status,
                size: exchange.response.body.size(),
            })
        } else {
            None
        }
    }

    /// Create a loading state with the current timestamp. This will generally
    /// be slightly off from when the request was actually launched, but it
    /// shouldn't matter. See [crate::http::RequestTicket::send] for why it
    /// can't report a start time back to us.
    pub fn loading(request: Arc<RequestRecord>) -> Self {
        Self::Loading {
            request,
            start_time: Utc::now(),
        }
    }

    /// Create a request state from a completed response. This is **expensive**,
    /// don't call it unless you need the value.
    pub fn response(exchange: Exchange) -> Self {
        // Pre-parse the body so the view doesn't have to do it. We're in the
        // main thread still here though so large bodies may take a while. Maybe
        // we want to punt this into a separate task?
        exchange.response.parse_body();
        Self::Response { exchange }
    }
}

/// A simplified version of [RequestState], which only stores metadata. This is
/// useful when you want to show a list of requests and don't need the entire
/// request/response data for each one.
#[derive(Debug)]
pub enum RequestStateSummary {
    Building {
        id: RequestId,
        start_time: DateTime<Utc>,
    },
    BuildError {
        id: RequestId,
        start_time: DateTime<Utc>,
        end_time: DateTime<Utc>,
    },
    Loading {
        id: RequestId,
        start_time: DateTime<Utc>,
    },
    Response(ExchangeSummary),
    RequestError {
        id: RequestId,
        time: DateTime<Utc>,
    },
}

impl RequestStateSummary {
    pub fn id(&self) -> RequestId {
        match self {
            Self::Building { id, .. }
            | Self::BuildError { id, .. }
            | Self::Loading { id, .. }
            | Self::RequestError { id, .. } => *id,
            Self::Response(exchange) => exchange.id,
        }
    }

    /// Get the time of the request state. For in-flight or completed requests,
    /// this is when it *started*.
    pub fn time(&self) -> DateTime<Utc> {
        match self {
            Self::Building {
                start_time: time, ..
            }
            | Self::BuildError {
                start_time: time, ..
            }
            | Self::Loading {
                start_time: time, ..
            }
            | Self::RequestError { time, .. } => *time,
            Self::Response(exchange) => exchange.start_time,
        }
    }
}

impl From<&RequestState> for RequestStateSummary {
    fn from(state: &RequestState) -> Self {
        match state {
            RequestState::Building { id, start_time, .. } => Self::Building {
                id: *id,
                start_time: *start_time,
            },
            RequestState::BuildError { error } => Self::BuildError {
                id: error.id,
                start_time: error.start_time,
                end_time: error.end_time,
            },
            RequestState::Loading {
                request,
                start_time,
                ..
            } => Self::Loading {
                id: request.id,
                start_time: *start_time,
            },
            RequestState::Response { exchange } => {
                Self::Response(exchange.into())
            }
            RequestState::RequestError { error } => Self::RequestError {
                id: error.request.id,
                time: error.start_time,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{harness, TestHarness};
    use anyhow::anyhow;
    use chrono::Utc;
    use rstest::rstest;
    use slumber_core::{
        assert_matches,
        http::{Exchange, RequestBuildError, RequestError, RequestRecord},
        test_util::Factory,
    };
    use std::sync::Arc;

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

    #[rstest]
    fn test_update() {
        let mut store = RequestStore::new(CollectionDatabase::factory(()));
        let exchange = Exchange::factory(());
        let id = exchange.id;

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
            store.load_latest(Some(&profile_id), &recipe_id).unwrap(),
            Some(&RequestState::response(expected_exchange))
        );

        // Non-match
        assert_matches!(
            store.load_latest(Some(&profile_id), &("other".into())),
            Ok(None)
        );
    }

    /// Test load_latest when the most recent request for the profile is a
    /// request that's not in the DB (i.e. in a state other than completed)
    #[rstest]
    fn test_load_latest_local(harness: TestHarness) {
        let mut store = harness.request_store.borrow_mut();
        let profile_id = ProfileId::factory(());
        let recipe_id = RecipeId::factory(());

        // We don't expect to load this one
        create_exchange(&harness, Some(&profile_id), Some(&recipe_id));

        // This is what we should see
        let request_record = Arc::new(RequestRecord::factory((
            Some(profile_id.clone()),
            recipe_id.clone(),
        )));

        store.update(RequestState::loading(Arc::clone(&request_record)));
        assert_eq!(
            store
                .load_latest(Some(&profile_id), &recipe_id)
                .unwrap()
                .map(RequestState::id),
            Some(request_record.id)
        );
    }

    #[rstest]
    fn test_load_summaries(harness: TestHarness) {
        let mut store = harness.request_store.borrow_mut();
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
                start_time: Utc::now(),
                end_time: Utc::now(),
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
