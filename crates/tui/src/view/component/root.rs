use crate::{
    http::{RequestState, RequestStore},
    message::{Message, RequestConfig},
    util::ResultReported,
    view::{
        Component, Confirm, ViewContext,
        common::{actions::ActionsMenu, modal::ModalQueue, text_box::TextBox},
        component::{
            Canvas, Child, ComponentId, Draw, DrawMetadata, ToChild,
            footer::Footer,
            history::History,
            internal::ComponentExt,
            misc::{ConfirmModal, ErrorModal, SelectListModal, TextBoxModal},
            primary::{PrimaryView, PrimaryViewProps},
        },
        context::UpdateContext,
        event::{Event, EventMatch},
        util::persistence::PersistedLazy,
    },
};
use derive_more::From;
use persisted::{PersistedContainer, PersistedKey};
use ratatui::{layout::Layout, prelude::Constraint};
use serde::Serialize;
use slumber_config::Action;
use slumber_core::{
    collection::{Collection, ProfileId},
    http::RequestId,
    render::{Prompt, Select},
};
use std::ops::Deref;

/// The root view component
#[derive(Debug)]
pub struct Root {
    // ===== Own State =====
    id: ComponentId,
    /// Which request are we showing in the request/response panel?
    selected_request_id: PersistedLazy<SelectedRequestKey, SelectedRequestId>,

    // ==== Children =====
    primary_view: PrimaryView,
    footer: Footer,
    // Modals!!
    // Some of these can only have one instance at a time while some are
    // queues. They use the same implementation, but it's only possible to open
    // multiple modals at a time if the trigger is from the background (e.g.
    // errors or prompts).
    actions: ActionsMenu,
    cancel_request_confirm: ModalQueue<ConfirmModal>,
    confirms: ModalQueue<ConfirmModal>,
    errors: ModalQueue<ErrorModal>,
    history: ModalQueue<History>,
    prompts: ModalQueue<TextBoxModal>,
    selects: ModalQueue<SelectListModal>,
}

impl Root {
    pub fn new(collection: &Collection) -> Self {
        // Load the selected request *second*, so it will take precedence over
        // the event that attempts to load the latest request for the recipe
        let selected_request_id: PersistedLazy<_, SelectedRequestId> =
            PersistedLazy::new_default(SelectedRequestKey);
        let primary_view = PrimaryView::new(collection);
        Self {
            id: ComponentId::default(),
            // State
            selected_request_id,

            // Children
            primary_view,
            footer: Footer::default(),
            actions: ActionsMenu::default(),
            cancel_request_confirm: ModalQueue::default(),
            confirms: ModalQueue::default(),
            errors: ModalQueue::default(),
            history: ModalQueue::default(),
            prompts: ModalQueue::default(),
            selects: ModalQueue::default(),
        }
    }

    /// Ask the user a yes/no question
    pub fn confirm(&mut self, confirm: Confirm) {
        self.confirms
            .open(ConfirmModal::new(confirm.message, |response| {
                confirm.channel.respond(response);
            }));
    }

    /// Display an error to the user
    pub fn error(&mut self, error: anyhow::Error) {
        self.errors.open(ErrorModal::new(error));
    }

    /// Display an informational message to the user
    pub fn notify(&mut self, message: String) {
        self.footer.notify(message);
    }

    /// Prompt the user for text input
    pub fn prompt(&mut self, prompt: Prompt) {
        self.prompts.open(TextBoxModal::new(
            prompt.message,
            TextBox::default()
                .sensitive(prompt.sensitive)
                .default_value(prompt.default.unwrap_or_default()),
            |response| prompt.channel.respond(response),
        ));
    }

    /// Ask the user to select an item from a list
    pub fn select(&mut self, select: Select) {
        self.selects.open(SelectListModal::new(
            select.message,
            select.options,
            |response| {
                select.channel.respond(response);
            },
        ));
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

    /// What request should be shown in the request/response pane right now?
    fn selected_request_id(&self) -> Option<RequestId> {
        self.selected_request_id.0
    }

    /// Extract the currently selected request from the store
    fn selected_request<'a>(
        &self,
        request_store: &'a RequestStore,
    ) -> Option<&'a RequestState> {
        self.selected_request_id()
            .and_then(|id| request_store.get(id))
    }

    /// Select the given request. This will ensure the request data is loaded
    /// in memory.
    pub fn select_request(
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
            let profile_id = self.primary_view.selected_profile_id();
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

        *self.selected_request_id.get_mut() =
            state.map(RequestState::id).into();
        Ok(())
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
                self.selected_request_id(),
            ));
        }
    }

    /// Cancel the active request
    fn cancel_request(&mut self, context: &mut UpdateContext<'_>) {
        if let Some(request_id) = self.selected_request_id.0
            && context.request_store.can_cancel(request_id)
        {
            self.cancel_request_confirm.open(ConfirmModal::new(
                "Cancel request?".into(),
                move |response| {
                    if response {
                        ViewContext::send_message(Message::HttpCancel(
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
                    let actions = self.primary_view.collect_actions();
                    // Actions can be empty if a modal is already open
                    if !actions.is_empty() {
                        self.actions.open(actions);
                    }
                }
                Action::History => self.open_history(context.request_store),
                Action::Cancel => self.cancel_request(context),
                Action::Quit => ViewContext::send_message(Message::Quit),
                Action::ReloadCollection => {
                    ViewContext::send_message(Message::CollectionStartReload);
                }
                _ => propagate.set(),
            })
            .any(|event| match event {
                // Set selected request, and load it from the DB if needed
                Event::HttpSelectRequest(request_id) => {
                    self.select_request(context.request_store, request_id)
                        .reported(&ViewContext::messages_tx());
                    None
                }

                // Any other unhandled input event should *not* log an error,
                // because it is probably just unmapped input, and not a bug
                Event::Input { .. } => None,

                // There shouldn't be anything left unhandled. Bubble up to log
                // it
                Event::Emitted { .. } => Some(event),
            })
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        vec![
            // Modals first. They won't eat events when closed
            // Error modal is always shown first, so it gets events first
            self.errors.to_child_mut(),
            // Rest of the modals
            self.actions.to_child_mut(),
            self.cancel_request_confirm.to_child_mut(),
            self.confirms.to_child_mut(),
            self.history.to_child_mut(),
            self.prompts.to_child_mut(),
            self.selects.to_child_mut(),
            // Non-modals
            self.primary_view.to_child_mut(),
            self.footer.to_child_mut(),
        ]
    }
}

impl<R: Deref<Target = RequestStore>> Draw<RootProps<R>> for Root {
    fn draw(
        &self,
        canvas: &mut Canvas,
        props: RootProps<R>,
        metadata: DrawMetadata,
    ) {
        // Create layout
        let [main_area, footer_area] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(1)])
                .areas(metadata.area());

        // Main content
        let selected_request = self
            .selected_request_id
            .0
            .and_then(|id| props.request_store.get(id));
        canvas.draw(
            &self.primary_view,
            PrimaryViewProps { selected_request },
            main_area,
            // If any modals are open, the modal queue will eat all input
            // events so we don't have to worry about catching stray events
            true,
        );

        // Footer
        canvas.draw(&self.footer, (), footer_area, false);

        // Modals
        canvas.draw_portal(&self.actions, (), true);
        canvas.draw_portal(&self.cancel_request_confirm, (), true);
        canvas.draw_portal(&self.confirms, (), true);
        canvas.draw_portal(&self.history, (), true);
        canvas.draw_portal(&self.prompts, (), true);
        canvas.draw_portal(&self.selects, (), true);
        // Errors render last because they're drawn on top (highest priority)
        canvas.draw_portal(&self.errors, (), true);
    }
}

#[derive(Debug)]
pub struct RootProps<R: Deref<Target = RequestStore>> {
    pub request_store: R,
}

/// Persistence key for the selected request
#[derive(Debug, Serialize, PersistedKey)]
#[persisted(Option<RequestId>)]
struct SelectedRequestKey;

/// A wrapper for the selected request ID. This is needed to customize
/// persistence loading. We have to load the persisted value via an event so it
/// can be loaded from the DB.
#[derive(Debug, Default, From)]
struct SelectedRequestId(Option<RequestId>);

impl PersistedContainer for SelectedRequestId {
    type Value = Option<RequestId>;

    fn get_to_persist(&self) -> Self::Value {
        self.0
    }

    fn restore_persisted(&mut self, request_id: Self::Value) {
        // We can't just set the value directly, because then the request won't
        // be loaded from the DB
        ViewContext::push_event(Event::HttpSelectRequest(request_id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_util::{TestHarness, TestTerminal, harness, terminal},
        view::{
            test_util::TestComponent, util::persistence::DatabasePersistedStore,
        },
    };
    use persisted::PersistedStore;
    use rstest::rstest;
    use slumber_core::{
        collection::{Profile, Recipe},
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

        let props_factory = || RootProps {
            request_store: harness.request_store.borrow(),
        };
        let mut component = TestComponent::builder(
            &harness,
            &terminal,
            Root::new(&harness.collection),
        )
        .with_props(props_factory())
        .build();
        component
            .int_props(props_factory)
            .drain_draw()
            .assert_empty();

        // Make sure profile+recipe were preselected correctly
        let primary_view = &component.primary_view;
        assert_eq!(primary_view.selected_profile_id(), Some(profile_id));
        assert_eq!(primary_view.selected_recipe_id(), Some(recipe_id));
        assert_eq!(component.selected_request_id(), Some(exchange.id));

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
        DatabasePersistedStore::store_persisted(
            &SelectedRequestKey,
            &Some(old_exchange.id),
        );

        let props_factory = || RootProps {
            request_store: harness.request_store.borrow(),
        };
        let mut component = TestComponent::builder(
            &harness,
            &terminal,
            Root::new(&harness.collection),
        )
        .with_props(props_factory())
        .build();
        component
            .int_props(props_factory)
            .drain_draw()
            .assert_empty();

        // Make sure everything was preselected correctly
        assert_eq!(
            component.primary_view.selected_profile_id(),
            Some(profile_id)
        );
        assert_eq!(
            component.primary_view.selected_recipe_id(),
            Some(recipe_id)
        );
        assert_eq!(component.selected_request_id(), Some(old_exchange.id));
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
        harness
            .database
            .set_ui(
                SelectedRequestKey::type_name(),
                &SelectedRequestKey,
                RequestId::new(),
            )
            .unwrap();

        let props_factory = || RootProps {
            request_store: harness.request_store.borrow(),
        };
        let mut component = TestComponent::builder(
            &harness,
            &terminal,
            Root::new(&harness.collection),
        )
        .with_props(props_factory())
        .build();
        component
            .int_props(props_factory)
            .drain_draw()
            .assert_empty();

        assert_eq!(component.selected_request_id(), Some(new_exchange.id));
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

        let props_factory = || RootProps {
            request_store: harness.request_store.borrow(),
        };
        let mut component = TestComponent::builder(
            &harness,
            &terminal,
            Root::new(&harness.collection),
        )
        .with_props(props_factory())
        .build();
        component
            .int_props(props_factory)
            .drain_draw()
            .assert_empty();

        assert_eq!(component.selected_request_id(), Some(exchange1.id));

        // Select the second recipe
        component
            .int_props(props_factory)
            .send_keys([KeyCode::Char('l'), KeyCode::Down])
            .assert_empty();
        assert_eq!(component.selected_request_id(), Some(exchange2.id));
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

        let props_factory = || RootProps {
            request_store: harness.request_store.borrow(),
        };
        let mut component = TestComponent::builder(
            &harness,
            &terminal,
            Root::new(&harness.collection),
        )
        .with_props(props_factory())
        .build();
        component
            .int_props(props_factory)
            .drain_draw()
            .assert_empty();

        assert_eq!(component.selected_request_id(), Some(exchange1.id));

        // Select the second profile
        component
            .int_props(props_factory)
            .send_keys([KeyCode::Char('p'), KeyCode::Down, KeyCode::Enter])
            .assert_empty();
        assert_eq!(component.selected_request_id(), Some(exchange2.id));
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

        let props_factory = || RootProps {
            request_store: harness.request_store.borrow(),
        };
        let mut component = TestComponent::builder(
            &harness,
            &terminal,
            Root::new(&harness.collection),
        )
        .with_props(props_factory())
        .build();
        // Select recipe pane
        component
            .int_props(props_factory)
            .drain_draw()
            .send_key(KeyCode::Char('c'))
            .assert_empty();

        // Sanity check for initial state
        assert_eq!(component.selected_request_id(), Some(new_exchange.id));

        // Select "Delete Requests" but decline the confirmation
        component
            .int_props(props_factory)
            .action("Delete Requests")
            // Decline
            .send_keys([KeyCode::Left, KeyCode::Enter])
            .assert_empty();

        // Same request is still selected
        assert_eq!(component.selected_request_id(), Some(new_exchange.id));

        // Select "Delete Requests" and accept. I don't feel like testing Delete
        // for All Profiles
        component
            .int_props(props_factory)
            .action("Delete Requests")
            // Confirm
            .send_keys([KeyCode::Enter])
            .assert_empty();

        assert_eq!(component.selected_request_id(), None);
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

        let props_factory = || RootProps {
            request_store: harness.request_store.borrow(),
        };
        let mut component = TestComponent::builder(
            &harness,
            &terminal,
            Root::new(&harness.collection),
        )
        .with_props(props_factory())
        .build();
        // Select exchange pane
        component
            .int_props(props_factory)
            .drain_draw()
            .send_key(KeyCode::Char('r'))
            .assert_empty();

        // Sanity check for initial state
        assert_eq!(component.selected_request_id(), Some(new_exchange.id));

        // Select "Delete Request" but decline the confirmation
        component
            .int_props(props_factory)
            .action("Delete Request")
            // Decline
            .send_keys([KeyCode::Left, KeyCode::Enter])
            .assert_empty();

        // Same request is still selected
        assert_eq!(component.selected_request_id(), Some(new_exchange.id));

        component
            .int_props(props_factory)
            .send_key(KeyCode::Char('r'))
            .action("Delete Request")
            // Confirm
            .send_keys([KeyCode::Enter])
            .assert_empty();

        // New exchange is gone
        assert_eq!(component.selected_request_id(), Some(old_exchange.id));
        assert_eq!(harness.request_store.borrow().get(new_exchange.id), None);
    }
}
