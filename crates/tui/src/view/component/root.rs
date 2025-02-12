use crate::{
    http::{RequestState, RequestStateSummary, RequestStore},
    message::{Message, RequestConfig},
    util::ResultReported,
    view::{
        common::{
            actions::ActionsModal,
            modal::{Modal, ModalQueue},
        },
        component::{
            help::HelpFooter,
            history::History,
            misc::{ConfirmModal, NotificationText},
            primary::{PrimaryView, PrimaryViewProps},
            profile_select::ProfileSelect,
            recipe_select::RecipeSelect,
        },
        context::UpdateContext,
        draw::{Draw, DrawMetadata, Generate},
        event::{Child, Event, EventHandler, OptionEvent},
        util::persistence::PersistedLazy,
        Component, ViewContext,
    },
};
use derive_more::From;
use persisted::{PersistedContainer, PersistedKey};
use ratatui::{layout::Layout, prelude::Constraint, Frame};
use serde::Serialize;
use slumber_config::Action;
use slumber_core::{
    collection::{Collection, ProfileId, RecipeId, RecipeNodeType},
    http::RequestId,
};

/// The root view component
#[derive(Debug)]
pub struct Root {
    // ===== Own State =====
    /// Which request are we showing in the request/response panel?
    selected_request_id: PersistedLazy<SelectedRequestKey, SelectedRequestId>,

    // ==== Children =====
    primary_view: Component<PrimaryView>,
    modal_queue: Component<ModalQueue>,
    profile_select: Component<ProfileSelect>,
    recipe_select: Component<RecipeSelect>,
    notification_text: Component<Option<NotificationText>>,
}

impl Root {
    pub fn new(collection: &Collection, request_store: &RequestStore) -> Self {
        // Load the selected request *second*, so it will take precedence over
        // the event that attempts to load the latest request for the recipe
        let selected_request_id: PersistedLazy<_, SelectedRequestId> =
            PersistedLazy::new_default(SelectedRequestKey);
        let selected_request =
            selected_request_id.0.and_then(|id| request_store.get(id));
        let primary_view = PrimaryView::new(selected_request);
        let profile_pane = ProfileSelect::new(collection);
        let recipe_list_pane = RecipeSelect::new(&collection.recipes);
        Self {
            // State
            selected_request_id,

            // Children
            primary_view: primary_view.into(),
            modal_queue: Component::default(),
            recipe_select: recipe_list_pane.into(),
            profile_select: profile_pane.into(),
            notification_text: Component::default(),
        }
    }

    /// ID of the selected profile. `None` iff the list is empty
    pub fn selected_profile_id(&self) -> Option<&ProfileId> {
        self.profile_select.data().selected_profile_id()
    }

    /// Which recipe in the recipe list is selected? `None` iff the list is
    /// empty OR a folder is selected.
    fn selected_recipe_id(&self) -> Option<&RecipeId> {
        self.recipe_select
            .data()
            .selected_node()
            .and_then(|(id, kind)| {
                if matches!(kind, RecipeNodeType::Recipe) {
                    Some(id)
                } else {
                    None
                }
            })
    }

    /// Get a definition of the request that should be sent from the current
    /// recipe settings
    pub fn request_config(&self) -> Option<RequestConfig> {
        self.primary_view.data().request_config()
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
        } else if let Some(recipe_id) = self.selected_recipe_id() {
            // We don't have a valid persisted ID, find the most recent for
            // the current recipe+profile

            // If someone asked for the latest request for a recipe, but we
            // already have another request of that same recipe selected,
            // ignore the request. This gets around a bug during
            // initialization where the recipe list asks for the latest
            // request *after* the selected ID is loaded from persistence
            let selected_request = self.selected_request(request_store);
            if selected_request
                .is_some_and(|request| request.recipe_id() == recipe_id)
            {
                selected_request
            } else {
                let profile_id = self.selected_profile_id();
                request_store.load_latest(profile_id, recipe_id)?
            }
        } else {
            None
        };

        *self.selected_request_id.get_mut() =
            state.map(RequestState::id).into();
        // Update view with the new request
        self.primary_view.data_mut().set_request_state(state);
        Ok(())
    }

    /// Notify the component that a request's state has changed in the store. If
    /// the request is selected, view state will be updated accordingly
    pub fn update_request(&mut self, request_state: &RequestState) {
        let selected_request_id = self.selected_request_id();
        if selected_request_id == Some(request_state.id()) {
            self.primary_view
                .data_mut()
                .set_request_state(Some(request_state));
        }
    }

    /// Open the history modal for current recipe+profile. Return an error if
    /// the harness.database load failed.
    fn open_history(
        &mut self,
        request_store: &mut RequestStore,
    ) -> anyhow::Result<()> {
        if let Some(recipe_id) = self.selected_recipe_id() {
            // Make sure all requests for this profile+recipe are loaded
            let requests = request_store
                .load_summaries(self.selected_profile_id(), recipe_id)?
                .map(RequestStateSummary::from)
                .collect();

            History::new(recipe_id, requests, self.selected_request_id())
                .open();
        }
        Ok(())
    }
}

impl EventHandler for Root {
    fn update(
        &mut self,
        context: &mut UpdateContext,
        event: Event,
    ) -> Option<Event> {
        event
            .opt()
            .action(|action, propagate| match action {
                Action::OpenActions => {
                    // Walk down the component tree and collect actions from
                    // all visible+focused components
                    let actions = self.primary_view.collect_actions();
                    ActionsModal::new(actions).open();
                }
                Action::History => {
                    self.open_history(context.request_store)
                        .reported(&ViewContext::messages_tx());
                }
                Action::Cancel => {
                    if let Some(request_id) = self.selected_request_id.0 {
                        // 2024 edition: if-let chain
                        if context.request_store.is_in_progress(request_id) {
                            ConfirmModal::new(
                                "Cancel request?".into(),
                                move |response| {
                                    if response {
                                        ViewContext::send_message(
                                            Message::HttpCancel(request_id),
                                        );
                                    }
                                },
                            )
                            .open()
                        }
                    }
                }
                Action::Quit => ViewContext::send_message(Message::Quit),
                Action::ReloadCollection => {
                    ViewContext::send_message(Message::CollectionStartReload)
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

                Event::Notify(notification) => {
                    self.notification_text =
                        Some(NotificationText::new(notification)).into();
                    None
                }

                // Any other unhandled input event should *not* log an error,
                // because it is probably just unmapped input, and not a bug
                Event::Input { .. } => None,

                // There shouldn't be anything left unhandled. Bubble up to log
                // it
                _ => Some(event),
            })
    }

    fn children(&mut self) -> Vec<Component<Child<'_>>> {
        vec![
            self.modal_queue.to_child_mut(),
            self.profile_select.to_child_mut(),
            self.recipe_select.to_child_mut(),
            self.primary_view.to_child_mut(),
        ]
    }
}

impl Draw for Root {
    fn draw(&self, frame: &mut Frame, _: (), metadata: DrawMetadata) {
        // Create layout
        let [header_area, main_area] =
            Layout::vertical([Constraint::Length(2), Constraint::Min(0)])
                .areas(metadata.area());

        // Header
        let [header_left_area, header_right_area] =
            Layout::horizontal([Constraint::Max(40), Constraint::Min(40)])
                .areas(header_area);
        let [profile_select_area, recipe_select_area] =
            Layout::vertical([Constraint::Length(1), Constraint::Length(1)])
                .areas(header_left_area);
        self.profile_select
            .draw(frame, (), profile_select_area, true);
        self.recipe_select.draw(frame, (), recipe_select_area, true);

        let footer = HelpFooter.generate();
        let [help_area, notification_area] =
            Layout::vertical([Constraint::Length(1), Constraint::Length(1)])
                .areas(header_right_area);
        self.notification_text
            .draw_opt(frame, (), notification_area, false);
        frame.render_widget(footer, help_area);

        // Main content
        let collection = ViewContext::collection();
        let selected_recipe_node = self
            .recipe_select
            .data()
            .selected_node()
            .and_then(|(id, _)| {
                collection
                    .recipes
                    .try_get(id)
                    .reported(&ViewContext::messages_tx())
            });
        self.primary_view.draw(
            frame,
            PrimaryViewProps {
                selected_recipe_node,
                selected_profile_id: self.selected_profile_id(),
            },
            main_area,
            !self.modal_queue.data().is_open(),
        );

        // Render modals last so they go on top
        self.modal_queue.draw(frame, (), frame.area(), true);
    }
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
        ViewContext::push_event(Event::HttpSelectRequest(request_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_util::{harness, terminal, TestHarness, TestTerminal},
        view::{
            test_util::TestComponent, util::persistence::DatabasePersistedStore,
        },
    };
    use persisted::PersistedStore;
    use rstest::rstest;
    use slumber_core::{http::Exchange, test_util::Factory};

    /// Test that, on first render, the view loads the most recent historical
    /// request for the first recipe+profile
    #[rstest]
    fn test_preload_request(harness: TestHarness, terminal: TestTerminal) {
        // Add a request into the DB that we expect to preload

        let collection = Collection::factory(());
        let profile_id = collection.first_profile_id();
        let recipe_id = collection.first_recipe_id();
        let exchange =
            Exchange::factory((Some(profile_id.clone()), recipe_id.clone()));
        harness.database.insert_exchange(&exchange).unwrap();

        let mut component = TestComponent::new(
            &harness,
            &terminal,
            Root::new(&collection, &harness.request_store.borrow()),
        );
        component.int().drain_draw().assert_empty();

        // Make sure profile+recipe were preselected correctly
        assert_eq!(component.data().selected_profile_id(), Some(profile_id));
        assert_eq!(component.data().selected_recipe_id(), Some(recipe_id));
        assert_eq!(component.data().selected_request_id(), Some(exchange.id));

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
        let collection = Collection::factory(());
        let recipe_id = collection.first_recipe_id();
        let profile_id = collection.first_profile_id();
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

        let mut component = TestComponent::new(
            &harness,
            &terminal,
            Root::new(&collection, &harness.request_store.borrow()),
        );
        component.int().drain_draw().assert_empty();

        // Make sure everything was preselected correctly
        assert_eq!(component.data().selected_profile_id(), Some(profile_id));
        assert_eq!(component.data().selected_recipe_id(), Some(recipe_id));
        assert_eq!(
            component.data().selected_request_id(),
            Some(old_exchange.id)
        );
    }

    /// Test that if the persisted request ID isn't in the DB, we'll fall back
    /// to selecting the most recent request
    #[rstest]
    fn test_persisted_request_missing(
        harness: TestHarness,
        terminal: TestTerminal,
    ) {
        let collection = Collection::factory(());
        let recipe_id = collection.first_recipe_id();
        let profile_id = collection.first_profile_id();
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

        let mut component = TestComponent::new(
            &harness,
            &terminal,
            Root::new(&collection, &harness.request_store.borrow()),
        );
        component.int().drain_draw().assert_empty();

        assert_eq!(
            component.data().selected_request_id(),
            Some(new_exchange.id)
        );
    }
}
