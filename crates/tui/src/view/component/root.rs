use crate::{
    http::{RequestConfig, RequestStore},
    message::{HttpMessage, Message},
    view::{
        Component, Generate, InvalidCollection, Question, RequestDisposition,
        ViewContext,
        common::{actions::ActionMenu, modal::ModalQueue},
        component::{
            Canvas, Child, ComponentId, Draw, DrawMetadata, ToChild,
            footer::Footer,
            internal::ComponentExt,
            misc::{ErrorModal, QuestionModal},
            primary::PrimaryView,
        },
        context::UpdateContext,
        event::{DeleteTarget, Event, EventMatch},
    },
};
use indexmap::IndexMap;
use ratatui::{layout::Layout, prelude::Constraint, text::Text};
use slumber_config::Action;
use slumber_core::{
    collection::{
        Collection, CollectionError, CollectionFile, HasId, Profile, ProfileId,
    },
    database::ProfileFilter,
};
use slumber_template::Template;
use std::{error::Error as StdError, sync::Arc};
use tracing::warn;

/// The root view component
#[derive(Debug)]
pub struct Root {
    id: ComponentId,
    /// The pane layout that forms the primary content
    ///
    /// The state of this is based on whether the collection loaded correctly.
    /// If it did, we can show the normal view. If the collection is invalid,
    /// show an error view until it's fixed.
    primary: Result<PrimaryView, CollectionErrorView>,
    footer: Footer,
    // Modals!!
    actions: ActionMenu,
    questions: ModalQueue<QuestionModal>,
    errors: ModalQueue<ErrorModal>,
}

impl Root {
    pub fn new(
        collection_result: Result<Arc<Collection>, InvalidCollection>,
    ) -> Self {
        let primary = match collection_result {
            Ok(_) => Ok(PrimaryView::new()),
            Err(invalid_collection) => {
                Err(CollectionErrorView::new(invalid_collection))
            }
        };
        Self {
            id: ComponentId::default(),
            primary,
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
        match &self.primary {
            Ok(primary) => primary.selected_profile_id(),
            Err(_) => None,
        }
    }

    /// Get a definition of the request that should be sent from the current
    /// recipe settings
    pub fn request_config(&self) -> Option<RequestConfig> {
        match &self.primary {
            Ok(primary) => primary.request_config(),
            Err(_) => None,
        }
    }

    /// Get a map of overridden profile fields
    pub fn profile_overrides(&self) -> IndexMap<String, Template> {
        match &self.primary {
            Ok(primary) => primary.profile_overrides(),
            Err(_) => IndexMap::default(),
        }
    }

    /// Update the UI to reflect the current state of an HTTP request
    pub fn refresh_request(
        &mut self,
        store: &mut RequestStore,
        disposition: RequestDisposition,
    ) {
        match &mut self.primary {
            Ok(primary) => primary.refresh_request(store, disposition),
            Err(_) => {}
        }
    }

    /// Open a modal to confirm deletion one or more requests
    fn delete_requests(&mut self, target: DeleteTarget) {
        let Ok(primary) = &mut self.primary else {
            return;
        };

        // Get the string to show to the user, and the message we should push
        // *if* they say yes
        let (title, message) = match target {
            DeleteTarget::Request => {
                let Some(request_id) = primary.selected_request_id() else {
                    // It shouldn't be possible to trigger this without a
                    // selected request
                    warn!("Cannot delete request; no request selected");
                    return;
                };

                (
                    "Delete Request?".into(),
                    HttpMessage::DeleteRequest(request_id),
                )
            }
            DeleteTarget::Recipe { all_profiles } => {
                let collection = ViewContext::collection();
                let Some(recipe) = primary
                    .selected_recipe_id()
                    .and_then(|id| collection.recipes.get_recipe(id))
                else {
                    // It shouldn't be possible to trigger this without a
                    // selected recipe
                    warn!("Cannot delete recipe requests; no recipe selected");
                    return;
                };
                let recipe_id = recipe.id.clone();

                // Check which profiles we should delete for
                if all_profiles {
                    // All profiles
                    (
                        format!(
                            "Delete all requests for {} (ALL profiles)?",
                            recipe.name()
                        ),
                        HttpMessage::DeleteRecipe {
                            recipe_id,
                            profile_filter: ProfileFilter::All,
                        },
                    )
                } else {
                    // Only a single profile. If there is no selected profile,
                    // we'll delete for profile=none
                    let profile = primary
                        .selected_profile_id()
                        .and_then(|id| collection.profiles.get(id));

                    (
                        format!(
                            "Delete all requests for {} (profile {})?",
                            recipe.name(),
                            profile.map(Profile::name).unwrap_or("None"),
                        ),
                        HttpMessage::DeleteRecipe {
                            recipe_id,
                            profile_filter: profile
                                .map(Profile::id)
                                .cloned()
                                .into(),
                        },
                    )
                }
            }
        };

        // Show a confirmation modal. If the user confirms, send the message
        // that will trigger the delete
        self.questions
            .open(QuestionModal::confirm(title, move |answer| {
                if answer {
                    ViewContext::send_message(message);
                }
            }));
    }

    /// Cancel the active request
    fn cancel_request(&mut self, context: &mut UpdateContext<'_>) {
        let Ok(primary) = &mut self.primary else {
            return;
        };

        if let Some(request_id) = primary.selected_request_id()
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
                    let actions = self.collect_actions(context);
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
        let primary = match &mut self.primary {
            Ok(primary) => primary.to_child_mut(),
            Err(error_view) => error_view.to_child_mut(),
        };
        vec![
            // Modals first. They won't eat events when closed
            // Error modal is always shown first, so it gets events first
            self.errors.to_child_mut(),
            // Rest of the modals
            self.actions.to_child_mut(),
            self.questions.to_child_mut(),
            // Non-modals
            // Footer has some high-priority pop-ups
            self.footer.to_child_mut(),
            primary,
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
        match &self.primary {
            Ok(primary) => canvas.draw(primary, (), main_area, true),
            Err(error_view) => canvas.draw(error_view, (), main_area, true),
        }

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

/// Display a collection load error
#[derive(Debug)]
struct CollectionErrorView {
    id: ComponentId,
    collection_file: CollectionFile,
    error: Arc<CollectionError>,
}

impl CollectionErrorView {
    fn new(invalid_collection: InvalidCollection) -> Self {
        Self {
            id: ComponentId::new(),
            collection_file: invalid_collection.file,
            error: invalid_collection.error,
        }
    }
}

impl Component for CollectionErrorView {
    fn id(&self) -> ComponentId {
        self.id
    }
}

impl Draw for CollectionErrorView {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        let [message_area, _, error_area] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Length(1), // A nice gap
            Constraint::Min(1),
        ])
        .areas(metadata.area());
        canvas.render_widget(
            (&*self.error as &dyn StdError).generate(),
            error_area,
        );
        canvas.render_widget(
            Text::styled(
                format!(
                    "Watching {file} for changes...\n{key} to exit",
                    file = self.collection_file,
                    key = ViewContext::binding_display(Action::ForceQuit),
                ),
                ViewContext::styles().text.primary,
            ),
            message_area,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_util::{TestTerminal, terminal},
        view::{
            component::history::SelectedRequestKey,
            test_util::{TestComponent, TestHarness, harness},
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

        let mut component = TestComponent::new(
            &harness,
            &terminal,
            Root::new(Ok(Arc::clone(&harness.collection))),
        );
        component.int().drain_draw().assert().empty();

        // Make sure profile+recipe were preselected correctly
        let primary = component.primary.as_ref().unwrap();
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

        let mut component = TestComponent::new(
            &harness,
            &terminal,
            Root::new(Ok(Arc::clone(&harness.collection))),
        );
        component.int().drain_draw().assert().empty();

        // Make sure everything was preselected correctly
        let primary = component.primary.as_ref().unwrap();
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

        let mut component = TestComponent::new(
            &harness,
            &terminal,
            Root::new(Ok(Arc::clone(&harness.collection))),
        );
        component.int().drain_draw().assert().empty();

        assert_eq!(
            component.primary.as_ref().unwrap().selected_request_id(),
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

        let mut component = TestComponent::new(
            &harness,
            &terminal,
            Root::new(Ok(Arc::clone(&harness.collection))),
        );
        component.int().drain_draw().assert().empty();

        assert_eq!(
            component.primary.as_ref().unwrap().selected_request_id(),
            Some(exchange1.id)
        );

        // Select the second recipe
        component
            .int()
            .send_keys([KeyCode::Char('r'), KeyCode::Down])
            .assert()
            .empty();
        assert_eq!(
            component.primary.as_ref().unwrap().selected_request_id(),
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

        let mut component = TestComponent::new(
            &harness,
            &terminal,
            Root::new(Ok(Arc::clone(&harness.collection))),
        );
        component.int().drain_draw().assert().empty();

        assert_eq!(
            component.primary.as_ref().unwrap().selected_request_id(),
            Some(exchange1.id)
        );

        // Select the second profile
        component
            .int()
            .send_keys([KeyCode::Char('p'), KeyCode::Down, KeyCode::Enter])
            .assert()
            .empty();
        // The exchange from profile2 should be selected now
        assert_eq!(
            component.primary.as_ref().unwrap().selected_request_id(),
            Some(exchange2.id)
        );
    }

    /// Test "Delete All Requests > This Profile" action via the recipe pane
    #[rstest]
    fn test_delete_recipe_requests(
        mut harness: TestHarness,
        #[with(120, 20)] terminal: TestTerminal,
    ) {
        let recipe_id = harness.collection.first_recipe_id().clone();
        let profile_id = harness.collection.first_profile_id().clone();
        let old_exchange =
            Exchange::factory((Some(profile_id.clone()), recipe_id.clone()));
        let new_exchange =
            Exchange::factory((Some(profile_id.clone()), recipe_id.clone()));
        harness.database.insert_exchange(&old_exchange).unwrap();
        harness.database.insert_exchange(&new_exchange).unwrap();

        let mut component = TestComponent::new(
            &harness,
            &terminal,
            Root::new(Ok(Arc::clone(&harness.collection))),
        );
        // Select History list
        component
            .int()
            .drain_draw()
            .send_key(KeyCode::Char('h'))
            .assert()
            .empty();

        // Sanity check for initial state
        assert_eq!(
            component.primary.as_ref().unwrap().selected_request_id(),
            Some(new_exchange.id)
        );

        // Select action but decline the confirmation
        let action_path = &["Delete All Requests", "This Profile"];
        component
            .int()
            .action(action_path)
            // Decline
            .send_keys([KeyCode::Left, KeyCode::Enter])
            .assert()
            .empty();

        // Same request is still selected
        assert_eq!(
            component.primary.as_ref().unwrap().selected_request_id(),
            Some(new_exchange.id)
        );

        // Select action and accept. I don't feel like testing All Profiles too
        component
            .int()
            .action(action_path)
            // Confirm
            .send_keys([KeyCode::Enter])
            .assert()
            .empty();

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

        let mut component = TestComponent::new(
            &harness,
            &terminal,
            Root::new(Ok(Arc::clone(&harness.collection))),
        );
        // Select exchange pane
        component
            .int()
            .drain_draw()
            .send_key(KeyCode::Char('2'))
            .assert()
            .empty();

        // Sanity check for initial state
        assert_eq!(
            component.primary.as_ref().unwrap().selected_request_id(),
            Some(new_exchange.id)
        );

        // Select "Delete Request" but decline the confirmation
        component
            .int()
            .action(&["Delete Request"])
            .send_keys([KeyCode::Left, KeyCode::Enter]) // Decline
            .assert()
            .empty();

        // Same request is still selected
        assert_eq!(
            component.primary.as_ref().unwrap().selected_request_id(),
            Some(new_exchange.id)
        );

        component
            .int()
            .action(&["Delete Request"])
            .send_keys([KeyCode::Enter]) // Confirm
            .assert()
            .empty();

        // It'd be nice to test that the request is actually deleted, but I
        // haven't figured out a way to test messages in the event loop.
        assert_matches!(
            harness.messages().pop_now(),
            Message::Http(HttpMessage::DeleteRequest(request_id))
                if request_id == new_exchange.id
        );
    }
}
