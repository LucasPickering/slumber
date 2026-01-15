use crate::{
    http::{RequestConfig, RequestStore},
    message::{HttpMessage, Message},
    view::{
        Component, Question, RequestDisposition, ViewContext,
        common::{actions::ActionMenu, modal::ModalQueue},
        component::{
            Canvas, Child, ComponentId, Draw, DrawMetadata, ToChild,
            footer::Footer,
            internal::ComponentExt,
            misc::{DeleteRequestsButton, ErrorModal, QuestionModal},
            primary::PrimaryView,
        },
        context::UpdateContext,
        event::{DeleteTarget, Event, EventMatch},
    },
};
use indexmap::IndexMap;
use ratatui::{layout::Layout, prelude::Constraint};
use slumber_config::Action;
use slumber_core::{collection::ProfileId, database::ProfileFilter};
use slumber_template::Template;
use tracing::warn;

/// The root view component
#[derive(Debug)]
pub struct Root {
    id: ComponentId,
    /// The pane layout that forms the primary content
    primary_view: PrimaryView,
    footer: Footer,
    // Modals!!
    actions: ActionMenu,
    questions: ModalQueue<QuestionModal>,
    errors: ModalQueue<ErrorModal>,
}

impl Root {
    pub fn new() -> Self {
        Self {
            id: ComponentId::default(),
            primary_view: PrimaryView::new(),
            footer: Footer::default(),
            actions: ActionMenu::default(),
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

    /// Update the UI to reflect the current state of an HTTP request
    pub fn refresh_request(
        &mut self,
        store: &mut RequestStore,
        disposition: RequestDisposition,
    ) {
        self.primary_view.refresh_request(store, disposition);
    }

    /// Open a modal to confirm deletion one or more requests
    fn delete_requests(&mut self, target: DeleteTarget) {
        match target {
            DeleteTarget::Request => {
                let Some(request_id) = self.primary_view.selected_request_id()
                else {
                    // It shouldn't be possible to trigger this without a
                    // selected request
                    warn!("Cannot delete request; no request selected");
                    return;
                };

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
            DeleteTarget::Recipe => {
                let Some(recipe_id) =
                    self.primary_view.selected_recipe_id().cloned()
                else {
                    // It shouldn't be possible to trigger this without a
                    // selected recipe
                    warn!("Cannot delete recipe request; no recipe selected");
                    return;
                };
                let profile_id =
                    self.primary_view.selected_profile_id().cloned();

                // Open a question modal to confirm deletion. We'll also ask
                // if they want to delete all requests for the recipe, or just
                // for this profile.
                self.questions.open(QuestionModal::delete_requests(
                    format!("Delete Requests for {recipe_id}?"),
                    move |button| {
                        // Do the delete here because we have access to the
                        // request store
                        let profile_filter = match button {
                            DeleteRequestsButton::No => None,
                            DeleteRequestsButton::Profile => {
                                Some(profile_id.into())
                            }
                            DeleteRequestsButton::All => {
                                Some(ProfileFilter::All)
                            }
                        };
                        if let Some(profile_filter) = profile_filter {
                            ViewContext::send_message(
                                HttpMessage::DeleteRecipe {
                                    recipe_id,
                                    profile_filter,
                                },
                            );
                        }
                    },
                ));
            }
        }
    }

    /// Cancel the active request
    fn cancel_request(&mut self, context: &mut UpdateContext<'_>) {
        if let Some(request_id) = self.primary_view.selected_request_id()
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
                // Broadcast events are *supposed* to be propagated!
                Event::Broadcast(_) => None,

                // Handle deletion here
                Event::DeleteRequests(target) => {
                    self.delete_requests(target);
                    None
                }

                // Ignore any emitted events that made it this far. It's
                // possible this event is indicative of a bug, but it's also
                // possible that it's been emitted by a component that has
                // since been dropped, in which case the event can safely be
                // ignored. Until we have an intelligent emitter handle that
                // will clear out its own events on drop, ignoring here saves
                // a lot of headaches.
                Event::Emitted { .. } => None,

                // Any other unhandled input event should *not* log an error,
                // because it is probably just unmapped input, and not a bug
                Event::Input { .. } => None,
            })
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        vec![
            // Modals first. They won't eat events when closed
            // Error modal is always shown first, so it gets events first
            self.errors.to_child_mut(),
            // Rest of the modals
            self.actions.to_child_mut(),
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

        // Draw modals/popups. These are all given the full screen area because
        // they want to capture all cursor events
        canvas.draw(&self.actions, (), metadata.area(), true);
        canvas.draw(&self.questions, (), metadata.area(), true);
        // Errors render last because they're drawn on top (highest priority)
        canvas.draw(&self.errors, (), metadata.area(), true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_util::{TestHarness, TestTerminal, harness, terminal},
        view::{
            component::history::SelectedRequestKey, test_util::TestComponent,
        },
    };
    use rstest::rstest;
    use slumber_core::{
        collection::{Collection, Profile, Recipe},
        http::{Exchange, RequestId},
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
        let primary = &component.primary_view;
        assert_eq!(primary.selected_profile_id(), Some(profile_id));
        assert_eq!(primary.selected_recipe_id(), Some(recipe_id));
        assert_eq!(primary.selected_request_id(), Some(exchange.id));

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
        let primary = &component.primary_view;
        assert_eq!(primary.selected_profile_id(), Some(profile_id));
        assert_eq!(primary.selected_recipe_id(), Some(recipe_id));
        assert_eq!(primary.selected_request_id(), Some(old_exchange.id));
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

        assert_eq!(
            component.primary_view.selected_request_id(),
            Some(new_exchange.id)
        );
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

        assert_eq!(
            component.primary_view.selected_request_id(),
            Some(exchange1.id)
        );

        // Select the second recipe
        component
            .int()
            .send_keys([KeyCode::Char('r'), KeyCode::Down])
            .assert_empty();
        assert_eq!(
            component.primary_view.selected_request_id(),
            Some(exchange2.id)
        );
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

        assert_eq!(
            component.primary_view.selected_request_id(),
            Some(exchange1.id)
        );

        // Select the second profile
        component
            .int()
            .send_keys([KeyCode::Char('p'), KeyCode::Down, KeyCode::Enter])
            .assert_empty();
        // The exchange from profile2 should be selected now
        assert_eq!(
            component.primary_view.selected_request_id(),
            Some(exchange2.id)
        );
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
        assert_eq!(
            component.primary_view.selected_request_id(),
            Some(new_exchange.id)
        );

        // Select "Delete Requests" but decline the confirmation
        component
            .int()
            .action(&["Delete Requests"])
            // Decline
            .send_keys([KeyCode::Left, KeyCode::Enter])
            .assert_empty();

        // Same request is still selected
        assert_eq!(
            component.primary_view.selected_request_id(),
            Some(new_exchange.id)
        );

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
    fn test_delete_request(
        mut harness: TestHarness,
        #[with(60, 20)] terminal: TestTerminal,
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
        // Select exchange pane
        component
            .int()
            .drain_draw()
            .send_key(KeyCode::Char('2'))
            .assert_empty();

        // Sanity check for initial state
        assert_eq!(
            component.primary_view.selected_request_id(),
            Some(new_exchange.id)
        );

        // Select "Delete Request" but decline the confirmation
        component
            .int()
            .action(&["Delete Request"])
            .send_keys([KeyCode::Left, KeyCode::Enter]) // Decline
            .assert_empty();

        // Same request is still selected
        assert_eq!(
            component.primary_view.selected_request_id(),
            Some(new_exchange.id)
        );

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
