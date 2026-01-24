use crate::{
    http::{RequestStateSummary, RequestStore},
    util::ResultReported,
    view::{
        Generate, UpdateContext, ViewContext,
        common::{
            Pane,
            actions::MenuItem,
            select::{Select, SelectEventKind, SelectListProps},
        },
        component::{
            Canvas, Component, ComponentId, Draw, DrawMetadata,
            internal::{Child, ToChild},
        },
        event::{
            BroadcastEvent, DeleteTarget, Emitter, Event, EventMatch, ToEmitter,
        },
        persistent::{PersistentKey, PersistentStore},
    },
};
use ratatui::text::{Line, Span, Text};
use serde::Serialize;
use slumber_config::Action;
use slumber_core::{
    collection::{ProfileId, RecipeId},
    http::RequestId,
};

/// Browse request/response history for a recipe
#[derive(Debug)]
pub struct History {
    id: ComponentId,
    actions_emitter: Emitter<HistoryAction>,
    select: Select<RequestStateSummary>,
    // We need to retain the selected profile/recipe IDs so we can access both
    // during a refresh. These are updated by the SelectedRecipe/Profile
    // broadcast events, so as long as those events are sent correctly, these
    // will stay in sync.
    selected_profile_id: Option<ProfileId>,
    selected_recipe_id: Option<RecipeId>,
}

impl History {
    /// Construct a new history modal with the given list of requests. Parent
    /// is responsible for loading the list from the request store.
    pub fn new(
        selected_profile_id: Option<ProfileId>,
        selected_recipe_id: Option<RecipeId>,
    ) -> Self {
        Self {
            id: ComponentId::default(),
            actions_emitter: Emitter::default(),
            // Always start with an empty list. On startup, we'll populate when
            // the initial SelectedRecipe/SelectedProfile events are received
            select: Self::build_select(vec![]),
            selected_profile_id,
            selected_recipe_id,
        }
    }

    /// Get the ID of the request that's currently selected. Return `None` iff
    /// the request list is empty
    pub fn selected_id(&self) -> Option<RequestId> {
        self.select.selected().map(RequestStateSummary::id)
    }

    /// Select a request by ID. Does nothing if the request ID is not in the
    /// current list.
    pub fn select_request(&mut self, id: RequestId) {
        self.select.select(&id);
    }

    /// Rebuild the request list from the store. This uses the retained
    /// profile/recipe IDs to query the DB for all matching requests
    pub fn refresh(&mut self, store: &mut RequestStore) {
        // If there's no recipe selected, there's no requests to show
        let requests = if let Some(recipe_id) = &self.selected_recipe_id {
            // Load matching requests from the DB
            store
                .load_summaries(self.selected_profile_id.as_ref(), recipe_id)
                .reported(&ViewContext::messages_tx())
                .map(Vec::from_iter)
                .unwrap_or_default()
        } else {
            vec![]
        };
        self.select = Self::build_select(requests);

        // If the list is empty, it never sends a Select event so we need to
        // manually notify our friends that there's no request selected.
        if self.select.is_empty() {
            ViewContext::push_event(BroadcastEvent::SelectedRequest(None));
        }
    }

    fn build_select(
        requests: Vec<RequestStateSummary>,
    ) -> Select<RequestStateSummary> {
        Select::builder(requests)
            .subscribe([SelectEventKind::Select])
            .persisted(&SelectedRequestKey)
            .build()
    }
}

impl Component for History {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn update(
        &mut self,
        context: &mut UpdateContext,
        event: Event,
    ) -> EventMatch {
        event
            .m()
            .action(|action, propagate| match action {
                Action::Delete => ViewContext::push_event(
                    Event::DeleteRequests(DeleteTarget::Request),
                ),
                _ => propagate.set(),
            })
            .emitted(self.actions_emitter, |menu_action| {
                let target = match menu_action {
                    HistoryAction::DeleteRequest => DeleteTarget::Request,
                    HistoryAction::DeleteRecipeProfile => {
                        DeleteTarget::Recipe {
                            all_profiles: false,
                        }
                    }
                    HistoryAction::DeleteRecipeAll => {
                        DeleteTarget::Recipe { all_profiles: true }
                    }
                };
                ViewContext::push_event(Event::DeleteRequests(target));
            })
            .emitted(self.select.to_emitter(), |event| match event.kind {
                SelectEventKind::Select => {
                    let id = self.select[event].id();
                    ViewContext::push_event(BroadcastEvent::SelectedRequest(
                        Some(id),
                    ));
                }
                SelectEventKind::Submit | SelectEventKind::Toggle => {}
            })
            .broadcast(|event| match event {
                // When the profile or recipe select changes, rebuild our list
                BroadcastEvent::SelectedProfile(profile_id) => {
                    self.selected_profile_id = profile_id;
                    self.refresh(context.request_store);
                }
                BroadcastEvent::SelectedRecipe(recipe_id) => {
                    self.selected_recipe_id = recipe_id;
                    self.refresh(context.request_store);
                }
                _ => {}
            })
    }

    fn menu(&self) -> Vec<MenuItem> {
        let emitter = self.actions_emitter;
        let has_requests = !self.select.is_empty();
        vec![
            emitter
                .menu(HistoryAction::DeleteRequest, "Delete Request")
                .shortcut(Some(Action::Delete))
                .enable(has_requests)
                .into(),
            MenuItem::Group {
                name: "Delete All Requests".into(),
                children: vec![
                    emitter
                        .menu(
                            HistoryAction::DeleteRecipeProfile,
                            "This Profile",
                        )
                        .enable(has_requests)
                        .into(),
                    emitter
                        .menu(HistoryAction::DeleteRecipeAll, "All Profiles")
                        .enable(has_requests)
                        .into(),
                ],
            },
        ]
    }

    fn persist(&self, store: &mut PersistentStore) {
        store.set_opt(&SelectedRequestKey, self.selected_id().as_ref());
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        vec![self.select.to_child_mut()]
    }
}

impl Draw for History {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        let title =
            ViewContext::add_binding_hint("Request History", Action::History);
        let block = Pane {
            title: &title,
            has_focus: metadata.has_focus(),
        }
        .generate();
        let area = block.inner(metadata.area());
        canvas.render_widget(block, metadata.area());

        if self.select.is_empty() {
            canvas.render_widget("No request history", area);
        } else {
            canvas.draw(&self.select, SelectListProps::modal(), area, true);
        }
    }
}

impl Generate for &RequestStateSummary {
    type Output<'this>
        = Text<'this>
    where
        Self: 'this;

    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this,
    {
        let styles = ViewContext::styles();
        let description: Span = match self {
            RequestStateSummary::Building { .. } => "Initializing...".into(),
            RequestStateSummary::BuildError { .. } => {
                Span::styled("Build error", styles.text.error)
            }
            RequestStateSummary::Loading { .. } => "Loading...".into(),
            RequestStateSummary::Cancelled { .. } => "Cancelled".into(),
            RequestStateSummary::Response(exchange) => {
                exchange.status.generate()
            }
            RequestStateSummary::RequestError { .. } => {
                Span::styled("Request error", styles.text.error)
            }
        };
        vec![
            Line::from_iter([
                self.start_time().generate(),
                " / ".into(),
                self.duration().generate(),
            ]),
            description.into(),
        ]
        .into()
    }
}

/// Allow selection by ID
impl PartialEq<RequestId> for RequestStateSummary {
    fn eq(&self, id: &RequestId) -> bool {
        &self.id() == id
    }
}

/// Persistence key for the selected request ID
///
/// Public so it can be used in the Root tests
#[derive(Debug, Serialize)]
pub struct SelectedRequestKey;

impl PersistentKey for SelectedRequestKey {
    type Value = RequestId;
}

#[derive(Copy, Clone, Debug)]
#[expect(clippy::enum_variant_names)]
enum HistoryAction {
    /// Delete the selected request
    DeleteRequest,
    /// Delete all requests for this recipe+profile
    DeleteRecipeProfile,
    /// Delete all requests for this recipe across all profiles
    DeleteRecipeAll,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_util::{TestTerminal, terminal},
        view::test_util::{TestComponent, TestHarness, harness},
    };
    use itertools::Itertools;
    use rstest::rstest;
    use slumber_core::http::Exchange;
    use slumber_util::Factory;
    use terminput::KeyCode;

    /// Test that we can browse requests, and selecting one updates root state
    #[rstest]
    fn test_navigation(harness: TestHarness, terminal: TestTerminal) {
        let profile_id = harness.collection.first_profile_id();
        let recipe_id = harness.collection.first_recipe_id();
        // Populate the DB
        let exchanges = (0..2)
            .map(|_| {
                Exchange::factory((Some(profile_id.clone()), recipe_id.clone()))
            })
            // Sort to match the modal
            .sorted_by_key(|exchange| exchange.start_time)
            .rev()
            .collect_vec();
        for exchange in &exchanges {
            harness.database.insert_exchange(exchange).unwrap();
        }

        let mut component = TestComponent::new(
            &harness,
            &terminal,
            History::new(Some(profile_id.clone()), Some(recipe_id.clone())),
        );
        // Normally the initial refresh is triggered by the Select events from
        // the profile/recipe list. We need to refresh manually here
        component.refresh(&mut harness.request_store_mut());

        // Initial state
        component.int().drain_draw().assert().broadcast([
            BroadcastEvent::SelectedRequest(Some(exchanges[0].id)),
        ]);

        // Select the next one
        component.int().send_key(KeyCode::Down).assert().broadcast([
            BroadcastEvent::SelectedRequest(Some(exchanges[1].id)),
        ]);
    }
}
