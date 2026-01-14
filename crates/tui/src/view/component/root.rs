use crate::{
    http::{RequestConfig, RequestState, RequestStore},
    message::{HttpMessage, Message},
    util::ResultReported,
    view::{
        Component, Question, RequestDisposition, ViewContext,
        common::{actions::ActionMenu, modal::ModalQueue},
        component::{
            Canvas, Child, ComponentId, Draw, DrawMetadata, ToChild,
            footer::Footer,
            internal::ComponentExt,
            misc::{DeleteRecipeRequestsModal, ErrorModal, QuestionModal},
            primary::PrimaryView,
        },
        context::UpdateContext,
        event::{DeleteTarget, Event, EventMatch},
        persistent::{PersistentKey, PersistentStore},
    },
};
use indexmap::IndexMap;
use ratatui::{layout::Layout, prelude::Constraint};
use serde::Serialize;
use slumber_config::Action;
use slumber_core::{collection::ProfileId, http::RequestId};
use slumber_template::Template;

/// The root view component
#[derive(Debug)]
pub struct Root {
    // ===== Own State =====
    id: ComponentId,
    /// Which request are we showing in the request/response panel?
    selected_request_id: Option<RequestId>,

    // ==== Children =====
    primary_view: PrimaryView,
    footer: Footer,
    // Modals!!
    // Some of these can only have one instance at a time while some are
    // queues. They use the same implementation, but it's only possible to open
    // multiple modals at a time if the trigger is from the background (e.g.
    // errors or prompts).
    actions: ActionMenu,
    /// Confirmation modal to delete all requests for a recipe
    delete_requests_confirm: ModalQueue<DeleteRecipeRequestsModal>,
    questions: ModalQueue<QuestionModal>,
    errors: ModalQueue<ErrorModal>,
}

impl Root {
    pub fn new() -> Self {
        // Restore the selected request via an event. When selecting the
        // request we need to load it into the request store as well, and we
        // don't have access to that here
        if let Some(selected_request_id) =
            PersistentStore::get(&SelectedRequestKey)
        {
            ViewContext::push_event(Event::HttpSelectRequest(Some(
                selected_request_id,
            )));
        }

        Self {
            id: ComponentId::default(),
            selected_request_id: None,

            // Children
            primary_view: PrimaryView::new(),
            footer: Footer::default(),
            actions: ActionMenu::default(),
            delete_requests_confirm: ModalQueue::default(),
            questions: ModalQueue::default(),
            errors: ModalQueue::default(),
        }
    }

    /// Ask the user a yes/no question
    pub fn question(&mut self, question: Question) {
        self.questions.open(QuestionModal::from_question(question));
    }

    /// Display an error to the user
    pub fn error(&mut self, error: anyhow::Error) {
        self.errors.open(ErrorModal::new(error));
    }

    /// Display an informational message to the user
    pub fn notify(&mut self, message: String) {
        self.footer.notify(message);
    }

    /// ID of the selected profile. `None` iff the list is empty
    pub fn selected_profile_id(&self) -> Option<&ProfileId> {
        self.primary_view.selected_profile_id()
    }

    /// Get a definition of the request that should be sent from the current
    /// recipe settings
    pub fn request_config(&self) -> Option<RequestConfig> {
        self.primary_view.request_config()
    }

    /// Get a map of overridden profile fields
    pub fn profile_overrides(&self) -> IndexMap<String, Template> {
        self.primary_view.profile_overrides()
    }

    /// Extract the currently selected request from the store
    fn selected_request<'a>(
        &self,
        request_store: &'a RequestStore,
    ) -> Option<&'a RequestState> {
        self.selected_request_id
            .and_then(|id| request_store.get(id))
    }

    /// Load a request from the store and select it
    fn load_and_select_request(
        &mut self,
        store: &mut RequestStore,
        request_id: Option<RequestId>,
    ) -> anyhow::Result<()> {
        let state = if let Some(request_id) = request_id {
            // TBH I would expect a bug here, if we're loading a persisted
            // request ID that doesn't exist anymore (e.g. we had a failed
            // request selected before exiting). But somehow we just fall back
            // to the most recent request for the recipe, as desired. I don't
            // understand it, but I'll take it...
            store.load(request_id)?
        } else if let Some(recipe_id) = self.primary_view.selected_recipe_id() {
            // If someone asked for the latest request for a recipe, but we
            // already have another request of that same recipe selected,
            // ignore the request. This gets around a bug during
            // initialization where the recipe list asks for the latest
            // request *after* the selected ID is loaded from persistence
            let selected_request = self.selected_request(store);
            let profile_id = self.selected_profile_id();
            if selected_request.is_some_and(|request| {
                request.recipe_id() == recipe_id
                    && request.profile_id() == profile_id
            }) {
                selected_request
            } else {
                store.load_latest(profile_id, recipe_id)?
            }
        } else {
            None
        };

        if let Some(state) = state {
            let id = state.id();
            self.refresh_request(store, RequestDisposition::Select(id));
        } else {
            // We switch to a recipe with no request, or just deleted the last
            // request for a recipe
            self.clear_request();
        }

        Ok(())
    }

    /// Update the UI to reflect the current state of an HTTP request. If
    /// `select` is `true`, select the request too.
    pub fn refresh_request(
        &mut self,
        store: &RequestStore,
        disposition: RequestDisposition,
    ) {
        match disposition {
            RequestDisposition::Change(request_id) => {
                // If the selected request was changed, rebuild state.
                // Otherwise, we don't care about the change
                if Some(request_id) == self.selected_request_id {
                    // If the request isn't in the store, that means it was just
                    // deleted
                    let state = store.get(request_id);
                    self.primary_view.set_request(state);
                }
            }
            RequestDisposition::ChangeAll(request_ids) => {
                // Check if the selected request changed
                if let Some(request_id) = self.selected_request_id
                    && request_ids.contains(&request_id)
                {
                    // If the request isn't in the store, that means it was just
                    // deleted
                    let state = store.get(request_id);
                    self.primary_view.set_request(state);
                }
            }
            RequestDisposition::Select(request_id) => {
                let Some(state) = store.get(request_id) else {
                    // If the request is not in the store, it can't be selected
                    return;
                };

                // Select only if it matches the current recipe/profile
                let selected_recipe_id = self.primary_view.selected_recipe_id();
                if state.profile_id() == self.selected_profile_id()
                    && Some(state.recipe_id()) == selected_recipe_id
                {
                    self.selected_request_id = Some(state.id());
                    self.primary_view.set_request(Some(state));
                }
            }
            RequestDisposition::OpenForm(request_id) => {
                // If a new prompt appears for a request that isn't selected, we
                // *don't* want to switch to it
                if Some(request_id) == self.selected_request_id {
                    // State *should* be Some here because the form just updated
                    let state = store.get(request_id);
                    // Update the view with the new prompt
                    self.primary_view.set_request(state);
                    // Select the form pane
                    self.primary_view.select_exchange_pane();
                }
            }
        }
    }

    /// Clear the selected request. Call this when switching to a state that has
    /// no request available
    fn clear_request(&mut self) {
        self.selected_request_id = None;
        self.primary_view.set_request(None);
    }

    /// Open a modal to confirm deletion one or more requests
    fn delete_requests(&mut self, target: DeleteTarget) {
        let profile_id = self.primary_view.selected_profile_id().cloned();
        match target {
            DeleteTarget::Request(request_id) => {
                self.questions.open(QuestionModal::confirm(
                    "Delete Request?".into(),
                    move |answer| {
                        if answer {
                            ViewContext::send_message(
                                HttpMessage::DeleteRequest(request_id),
                            );
                        }
                    },
                ));
            }
            DeleteTarget::Recipe(recipe_id) => {
                self.delete_requests_confirm.open(
                    DeleteRecipeRequestsModal::new(profile_id, recipe_id),
                );
            }
        }
    }

    /// Cancel the active request
    fn cancel_request(&mut self, context: &mut UpdateContext<'_>) {
        if let Some(request_id) = self.selected_request_id
            && context.request_store.can_cancel(request_id)
        {
            self.questions.open(QuestionModal::confirm(
                "Cancel request?".into(),
                move |answer| {
                    if answer {
                        ViewContext::send_message(HttpMessage::Cancel(
                            request_id,
                        ));
                    }
                },
            ));
        }
    }
}

impl Component for Root {
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
                Action::Cancel => self.cancel_request(context),
                Action::History => {
                    // We have to handle this event here because we have the
                    // selected request ID
                    self.primary_view.open_history(
                        context.request_store,
                        self.selected_request_id,
                    );
                }
                Action::OpenActions => {
                    // Walk down the component tree and collect actions from
                    // all visible+focused components
                    let actions = self.primary_view.collect_actions(context);
                    // Actions can be empty if a modal is already open
                    if !actions.is_empty() {
                        self.actions.open(actions);
                    }
                }
                Action::Quit => ViewContext::send_message(Message::Quit),
                Action::ReloadCollection => {
                    ViewContext::send_message(Message::CollectionStartReload);
                }
                _ => propagate.set(),
            })
            .any(|event| match event {
                Event::DeleteRequests(target) => {
                    self.delete_requests(target);
                    None
                }

                Event::RefreshPreviews => {
                    // This is broadcast to all template previews. They all
                    // propagate to allow their friends to view it; we eat it
                    // here just so it doesn't trigger a warning.
                    None
                }

                // There shouldn't be anything left unhandled. Bubble up to log
                // it
                Event::Emitted { .. } => Some(event),

                // Set selected request, and load it from the DB if needed
                Event::HttpSelectRequest(request_id) => {
                    self.load_and_select_request(
                        context.request_store,
                        request_id,
                    )
                    .reported(&ViewContext::messages_tx());
                    None
                }

                // Any other unhandled input event should *not* log an error,
                // because it is probably just unmapped input, and not a bug
                Event::Input { .. } => None,
            })
    }

    fn persist(&self, store: &mut PersistentStore) {
        store.set_opt(&SelectedRequestKey, self.selected_request_id.as_ref());
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        vec![
            // Modals first. They won't eat events when closed
            // Error modal is always shown first, so it gets events first
            self.errors.to_child_mut(),
            // Rest of the modals
            self.actions.to_child_mut(),
            self.delete_requests_confirm.to_child_mut(),
            self.questions.to_child_mut(),
            // Footer has some high-priority pop-ups
            self.footer.to_child_mut(),
            // Non-modals
            self.primary_view.to_child_mut(),
        ]
    }
}

impl Draw for Root {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        // Create layout
        let [main_area, footer_area] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(1)])
                .areas(metadata.area());

        // Main content
        canvas.draw(
            &self.primary_view,
            (),
            main_area,
            // If any modals are open, the modal queue will eat all input
            // events so we don't have to worry about catching stray events
            true,
        );

        // Footer
        canvas.draw(&self.footer, (), footer_area, true);

        // Modals
        canvas.draw_portal(&self.actions, (), true);
        canvas.draw_portal(&self.delete_requests_confirm, (), true);
        canvas.draw_portal(&self.questions, (), true);
        // Errors render last because they're drawn on top (highest priority)
        canvas.draw_portal(&self.errors, (), true);
    }
}

/// Persistence key for the selected request
#[derive(Debug, Serialize)]
struct SelectedRequestKey;

impl PersistentKey for SelectedRequestKey {
    type Value = RequestId;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_util::{TestHarness, TestTerminal, harness, terminal},
        view::test_util::TestComponent,
    };
    use rstest::rstest;
    use slumber_core::{
        collection::{Collection, Profile, Recipe},
        http::Exchange,
        test_util::by_id,
    };
    use slumber_util::{Factory, assert_matches};
    use terminput::KeyCode;

    /// Test that, on first render, the view loads the most recent historical
    /// request for the first recipe+profile
    #[rstest]
    fn test_preload_request(harness: TestHarness, terminal: TestTerminal) {
        // Add a request into the DB that we expect to preload
        let profile_id = harness.collection.first_profile_id();
        let recipe_id = harness.collection.first_recipe_id();
        let exchange =
            Exchange::factory((Some(profile_id.clone()), recipe_id.clone()));
        harness.database.insert_exchange(&exchange).unwrap();

        let mut component =
            TestComponent::new(&harness, &terminal, Root::new());
        component.int().drain_draw().assert_empty();

        // Make sure profile+recipe were preselected correctly
        let primary_view = &component.primary_view;
        assert_eq!(primary_view.selected_profile_id(), Some(profile_id));
        assert_eq!(primary_view.selected_recipe_id(), Some(recipe_id));
        assert_eq!(component.selected_request_id, Some(exchange.id));

        // It'd be nice to assert on the view but it's just too complicated to
        // be worth mocking the whole thing out
    }

    /// Test that, on first render, if there's a persisted request ID, we load
    /// up to that instead of selecting the first in the list
    #[rstest]
    fn test_load_persisted_request(
        harness: TestHarness,
        terminal: TestTerminal,
    ) {
        let recipe_id = harness.collection.first_recipe_id();
        let profile_id = harness.collection.first_profile_id();
        // This is the older one, but it should be loaded because of persistence
        let old_exchange =
            Exchange::factory((Some(profile_id.clone()), recipe_id.clone()));
        let new_exchange =
            Exchange::factory((Some(profile_id.clone()), recipe_id.clone()));
        harness.database.insert_exchange(&old_exchange).unwrap();
        harness.database.insert_exchange(&new_exchange).unwrap();
        harness
            .persistent_store()
            .set(&SelectedRequestKey, &old_exchange.id);

        let mut component =
            TestComponent::new(&harness, &terminal, Root::new());
        component.int().drain_draw().assert_empty();

        // Make sure everything was preselected correctly
        assert_eq!(
            component.primary_view.selected_profile_id(),
            Some(profile_id)
        );
        assert_eq!(
            component.primary_view.selected_recipe_id(),
            Some(recipe_id)
        );
        assert_eq!(component.selected_request_id, Some(old_exchange.id));
    }

    /// Test that if the persisted request ID isn't in the DB, we'll fall back
    /// to selecting the most recent request
    #[rstest]
    fn test_persisted_request_missing(
        harness: TestHarness,
        terminal: TestTerminal,
    ) {
        let recipe_id = harness.collection.first_recipe_id();
        let profile_id = harness.collection.first_profile_id();
        let old_exchange =
            Exchange::factory((Some(profile_id.clone()), recipe_id.clone()));
        let new_exchange =
            Exchange::factory((Some(profile_id.clone()), recipe_id.clone()));
        harness.database.insert_exchange(&old_exchange).unwrap();
        harness.database.insert_exchange(&new_exchange).unwrap();
        // Put a random ID in the DB
        harness
            .persistent_store()
            .set(&SelectedRequestKey, &RequestId::new());

        let mut component =
            TestComponent::new(&harness, &terminal, Root::new());
        component.int().drain_draw().assert_empty();

        assert_eq!(component.selected_request_id, Some(new_exchange.id));
    }

    /// Test that when the selected recipe changes, the selected request changes
    /// as well
    #[rstest]
    fn test_recipe_change(terminal: TestTerminal) {
        let recipe1 = Recipe::factory(());
        let recipe2 = Recipe::factory(());
        let recipe1_id = recipe1.id.clone();
        let recipe2_id = recipe2.id.clone();
        let collection = Collection {
            recipes: by_id([recipe1, recipe2]).into(),
            ..Collection::factory(())
        };
        let harness = TestHarness::new(collection);
        let profile_id = harness.collection.first_profile_id();
        let exchange1 =
            Exchange::factory((Some(profile_id.clone()), recipe1_id.clone()));
        let exchange2 =
            Exchange::factory((Some(profile_id.clone()), recipe2_id.clone()));
        harness.database.insert_exchange(&exchange1).unwrap();
        harness.database.insert_exchange(&exchange2).unwrap();

        let mut component =
            TestComponent::new(&harness, &terminal, Root::new());
        component.int().drain_draw().assert_empty();

        assert_eq!(component.selected_request_id, Some(exchange1.id));

        // Select the second recipe
        component
            .int()
            .send_keys([KeyCode::Char('r'), KeyCode::Down])
            .assert_empty();
        assert_eq!(component.selected_request_id, Some(exchange2.id));
    }

    /// Test that when the selected profile changes, the selected request
    /// changes as well
    #[rstest]
    fn test_profile_change(terminal: TestTerminal) {
        let profile1 = Profile::factory(());
        let profile2 = Profile::factory(());
        let profile1_id = profile1.id.clone();
        let profile2_id = profile2.id.clone();
        let collection = Collection {
            profiles: by_id([profile1, profile2]),
            ..Collection::factory(())
        };
        let harness = TestHarness::new(collection);
        let recipe_id = harness.collection.first_recipe_id();
        let exchange1 =
            Exchange::factory((Some(profile1_id.clone()), recipe_id.clone()));
        let exchange2 =
            Exchange::factory((Some(profile2_id.clone()), recipe_id.clone()));
        harness.database.insert_exchange(&exchange1).unwrap();
        harness.database.insert_exchange(&exchange2).unwrap();

        let mut component =
            TestComponent::new(&harness, &terminal, Root::new());
        component.int().drain_draw().assert_empty();

        assert_eq!(component.selected_request_id, Some(exchange1.id));

        // Select the second profile
        component
            .int()
            .send_keys([KeyCode::Char('p'), KeyCode::Down, KeyCode::Enter])
            .assert_empty();
        // The exchange from profile2 should be selected now
        assert_eq!(component.selected_request_id, Some(exchange2.id));
    }

    /// Test "Delete Requests" action via the recipe pane
    #[rstest]
    fn test_delete_recipe_requests(
        mut harness: TestHarness,
        #[with(80, 20)] terminal: TestTerminal,
    ) {
        let recipe_id = harness.collection.first_recipe_id().clone();
        let profile_id = harness.collection.first_profile_id().clone();
        let old_exchange =
            Exchange::factory((Some(profile_id.clone()), recipe_id.clone()));
        let new_exchange =
            Exchange::factory((Some(profile_id.clone()), recipe_id.clone()));
        harness.database.insert_exchange(&old_exchange).unwrap();
        harness.database.insert_exchange(&new_exchange).unwrap();

        let mut component =
            TestComponent::new(&harness, &terminal, Root::new());
        // Select recipe pane
        component
            .int()
            .drain_draw()
            .send_key(KeyCode::Char('c'))
            .assert_empty();

        // Sanity check for initial state
        assert_eq!(component.selected_request_id, Some(new_exchange.id));

        // Select "Delete Requests" but decline the confirmation
        component
            .int()
            .action(&["Delete Requests"])
            // Decline
            .send_keys([KeyCode::Left, KeyCode::Enter])
            .assert_empty();

        // Same request is still selected
        assert_eq!(component.selected_request_id, Some(new_exchange.id));

        // Select "Delete Requests" and accept. I don't feel like testing Delete
        // for All Profiles
        component
            .int()
            .action(&["Delete Requests"])
            // Confirm
            .send_keys([KeyCode::Enter])
            .assert_empty();

        // It'd be nice to test that the request is actually deleted, but I
        // haven't figured out a way to test messages in the event loop.
        assert_matches!(
            harness.messages().pop_now(),
            Message::Http(HttpMessage::DeleteRecipe {
                recipe_id: ref rid,
                profile_filter: ref pf
            }) if rid == &recipe_id && pf == &Some(profile_id).into()
        );
    }

    /// Test "Delete Request" action, which is available via the
    /// Request/Response pane
    #[rstest]
    fn test_delete_request(mut harness: TestHarness, terminal: TestTerminal) {
        let recipe_id = harness.collection.first_recipe_id();
        let profile_id = harness.collection.first_profile_id();
        let old_exchange =
            Exchange::factory((Some(profile_id.clone()), recipe_id.clone()));
        let new_exchange =
            Exchange::factory((Some(profile_id.clone()), recipe_id.clone()));
        harness.database.insert_exchange(&old_exchange).unwrap();
        harness.database.insert_exchange(&new_exchange).unwrap();

        let mut component =
            TestComponent::new(&harness, &terminal, Root::new());
        // Select exchange pane
        component
            .int()
            .drain_draw()
            .send_key(KeyCode::Char('2'))
            .assert_empty();

        // Sanity check for initial state
        assert_eq!(component.selected_request_id, Some(new_exchange.id));

        // Select "Delete Request" but decline the confirmation
        component
            .int()
            .action(&["Delete Request"])
            .send_keys([KeyCode::Left, KeyCode::Enter]) // Decline
            .assert_empty();

        // Same request is still selected
        assert_eq!(component.selected_request_id, Some(new_exchange.id));

        component
            .int()
            .action(&["Delete Request"])
            .send_keys([KeyCode::Enter]) // Confirm
            .assert_empty();

        // It'd be nice to test that the request is actually deleted, but I
        // haven't figured out a way to test messages in the event loop.
        assert_matches!(
            harness.messages().pop_now(),
            Message::Http(HttpMessage::DeleteRequest(request_id))
                if request_id == new_exchange.id
        );
    }
}
