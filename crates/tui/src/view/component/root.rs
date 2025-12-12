use crate::{
    http::{RequestConfig, RequestState, RequestStore},
    message::{HttpMessage, Message},
    util::ResultReported,
    view::{
        Component, Question, ViewContext,
        common::{actions::ActionMenu, modal::ModalQueue},
        component::{
            Canvas, Child, ComponentId, Draw, DrawMetadata, ToChild,
            footer::Footer,
            help::Help,
            history::History,
            internal::ComponentExt,
            misc::{DeleteRecipeRequestsModal, ErrorModal, QuestionModal},
            primary::PrimaryView,
        },
        context::UpdateContext,
        event::{Event, EventMatch},
        persistent::{PersistentKey, PersistentStore},
    },
};
use ratatui::{layout::Layout, prelude::Constraint};
use serde::Serialize;
use slumber_config::Action;
use slumber_core::{collection::ProfileId, http::RequestId};

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
    /// Fullscreen help page. When open, it'll be the only thing we draw
    help: Help,
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
    history: ModalQueue<History>,
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
            help: Help::default(),
            actions: ActionMenu::default(),
            delete_requests_confirm: ModalQueue::default(),
            questions: ModalQueue::default(),
            errors: ModalQueue::default(),
            history: ModalQueue::default(),
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
        request_store: &mut RequestStore,
        request_id: Option<RequestId>,
    ) -> anyhow::Result<()> {
        let state = if let Some(request_id) = request_id {
            // TBH I would expect a bug here, if we're loading a persisted
            // request ID that doesn't exist anymore (e.g. we had a failed
            // request selected before exiting). But somehow we just fall back
            // to the most recent request for the recipe, as desired. I don't
            // understand it, but I'll take it...
            request_store.load(request_id)?
        } else if let Some(recipe_id) = self.primary_view.selected_recipe_id() {
            // We don't have a valid persisted ID, find the most recent for
            // the current recipe+profile

            // If someone asked for the latest request for a recipe, but we
            // already have another request of that same recipe selected,
            // ignore the request. This gets around a bug during
            // initialization where the recipe list asks for the latest
            // request *after* the selected ID is loaded from persistence
            let selected_request = self.selected_request(request_store);
            let profile_id = self.selected_profile_id();
            if selected_request.is_some_and(|request| {
                request.recipe_id() == recipe_id
                    && request.profile_id() == profile_id
            }) {
                selected_request
            } else {
                request_store.load_latest(profile_id, recipe_id)?
            }
        } else {
            None
        };

        if let Some(state) = state {
            self.update_request(state, true);
        } else {
            // We switch to a recipe with no request, or just deleted the last
            // request for a recipe
            self.clear_request();
        }

        Ok(())
    }

    /// Update the UI to reflect the current state of an HTTP request. If
    /// `select` is `true`, select the request too.
    pub fn update_request(&mut self, state: &RequestState, select: bool) {
        // If we're being told to select a request but it's not for the current
        // profile/recipe, then we say: NO. YOU'RE NOT MY REAL MOM.
        if select
            && state.profile_id() == self.selected_profile_id()
            && Some(state.recipe_id()) == self.primary_view.selected_recipe_id()
        {
            self.selected_request_id = Some(state.id());
        }

        // If the updated request is the one in view, rebuild the view
        if Some(state.id()) == self.selected_request_id {
            self.primary_view.refresh_request(Some(state));
        }
    }

    /// Clear the selected request. Call this when switching to a state that has
    /// no request available
    fn clear_request(&mut self) {
        self.selected_request_id = None;
        self.primary_view.refresh_request(None);
    }

    /// Open the history modal for current recipe+profile
    fn open_history(&mut self, request_store: &mut RequestStore) {
        if let Some(recipe_id) = self.primary_view.selected_recipe_id() {
            // Make sure all requests for this profile+recipe are loaded
            let requests = request_store
                .load_summaries(
                    self.primary_view.selected_profile_id(),
                    recipe_id,
                )
                .reported(&ViewContext::messages_tx())
                .map(Vec::from_iter)
                .unwrap_or_default();

            self.history.open(History::new(
                recipe_id,
                requests,
                self.selected_request_id,
            ));
        }
    }

    /// Open a modal to confirm deletion of all requests for the selected recipe
    fn delete_requests(&mut self) {
        if let Some(recipe_id) = self.primary_view.selected_recipe_id().cloned()
        {
            let profile_id = self.primary_view.selected_profile_id().cloned();
            self.delete_requests_confirm
                .open(DeleteRecipeRequestsModal::new(profile_id, recipe_id));
        }
    }

    /// Cancel the active request
    fn cancel_request(&mut self, context: &mut UpdateContext<'_>) {
        if let Some(request_id) = self.selected_request_id
            && context.request_store.can_cancel(request_id)
        {
            self.questions.open(QuestionModal::confirm(
                "Cancel request?".into(),
                move |response| {
                    if response {
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
                Action::OpenActions => {
                    // Walk down the component tree and collect actions from
                    // all visible+focused components
                    let actions = self.primary_view.collect_actions(context);
                    // Actions can be empty if a modal is already open
                    if !actions.is_empty() {
                        self.actions.open(actions);
                    }
                }
                Action::OpenHelp => self.help.open(),
                Action::History => self.open_history(context.request_store),
                Action::Cancel => self.cancel_request(context),
                Action::Quit => ViewContext::send_message(Message::Quit),
                Action::ReloadCollection => {
                    ViewContext::send_message(Message::CollectionStartReload);
                }
                _ => propagate.set(),
            })
            .any(|event| match event {
                Event::DeleteRecipeRequests => {
                    self.delete_requests();
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
            self.history.to_child_mut(),
            // Non-modals
            self.primary_view.to_child_mut(),
            self.footer.to_child_mut(),
            self.help.to_child_mut(),
        ]
    }
}

impl Draw for Root {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        // Help is a full-page thing and eats all actions, so if it's open it's
        // the only thing we'll draw
        if self.help.is_open() {
            canvas.draw(&self.help, (), metadata.area(), true);
            return;
        }

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
        canvas.draw(&self.footer, (), footer_area, false);

        // Modals
        canvas.draw_portal(&self.actions, (), true);
        canvas.draw_portal(&self.delete_requests_confirm, (), true);
        canvas.draw_portal(&self.questions, (), true);
        canvas.draw_portal(&self.history, (), true);
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
    use slumber_util::Factory;
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
            .send_keys([KeyCode::Char('2'), KeyCode::Down])
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
            .send_keys([KeyCode::Char('1'), KeyCode::Down, KeyCode::Enter])
            .assert_empty();
        // The exchange from profile2 should be selected now
        assert_eq!(component.selected_request_id, Some(exchange2.id));
    }

    /// Test "Delete Requests" action via both the recipe pane
    #[rstest]
    fn test_delete_recipe_requests(
        harness: TestHarness,
        #[with(80, 20)] terminal: TestTerminal,
    ) {
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

        assert_eq!(component.selected_request_id, None);
    }

    /// Test "Delete Request" action, which is available via the
    /// Request/Response pane
    #[rstest]
    fn test_delete_request(harness: TestHarness, terminal: TestTerminal) {
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
            .send_key(KeyCode::Char('r'))
            .assert_empty();

        // Sanity check for initial state
        assert_eq!(component.selected_request_id, Some(new_exchange.id));

        // Select "Delete Request" but decline the confirmation
        component
            .int()
            .action(&["Delete Request"])
            // Decline
            .send_keys([KeyCode::Left, KeyCode::Enter])
            .assert_empty();

        // Same request is still selected
        assert_eq!(component.selected_request_id, Some(new_exchange.id));

        component
            .int()
            .send_key(KeyCode::Char('r'))
            .action(&["Delete Request"])
            // Confirm
            .send_keys([KeyCode::Enter])
            .assert_empty();

        // New exchange is gone
        assert_eq!(component.selected_request_id, Some(old_exchange.id));
        assert_eq!(harness.request_store.borrow().get(new_exchange.id), None);
    }

    /// Open and close the help page
    #[rstest]
    fn test_help(harness: TestHarness, terminal: TestTerminal) {
        let mut component =
            TestComponent::new(&harness, &terminal, Root::new());
        assert!(!component.help.is_open());

        // Open help
        component
            .int()
            .drain_draw() // Clear initial events
            .send_key(KeyCode::Char('?'))
            .assert_empty();
        assert!(component.help.is_open());

        // Any key should close. Events are *not* handled by anyone else
        //
        component.int().send_key(KeyCode::Char('x')).assert_empty();
        assert!(!component.help.is_open());
    }
}
