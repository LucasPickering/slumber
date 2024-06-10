use crate::{
    collection::Collection,
    http::RequestId,
    tui::{
        input::Action,
        message::Message,
        view::{
            common::{actions::GlobalAction, modal::ModalQueue},
            component::{
                help::HelpFooter,
                history::History,
                misc::NotificationText,
                primary::{PrimaryView, PrimaryViewProps},
            },
            draw::{Draw, DrawMetadata, Generate},
            event::{Event, EventHandler, Update},
            state::{
                persistence::{
                    Persistable, Persistent, PersistentContainer, PersistentKey,
                },
                request_store::RequestStore,
                RequestState, RequestStateSummary,
            },
            Component, ModalPriority, ViewContext,
        },
    },
    util::ResultExt,
};
use derive_more::{Deref, DerefMut};
use ratatui::{layout::Layout, prelude::Constraint, Frame};

/// The root view component
#[derive(Debug)]
pub struct Root {
    // ===== Own State =====
    /// Track and cache in-progress and completed requests
    request_store: RequestStore,
    /// Which request are we showing in the request/response panel?
    selected_request: Persistent<SelectedRequestId>,

    // ==== Children =====
    /// We hold onto the primary view even when it's not visible, because we
    /// don't want the state to reset when changing views
    primary_view: Component<PrimaryView>,
    modal_queue: Component<ModalQueue>,
    notification_text: Option<Component<NotificationText>>,
}

impl Root {
    pub fn new(collection: &Collection) -> Self {
        // Load the selected request *second*, so it will take precedence over
        // the event that attempts to load the latest request for the recipe
        let primary_view = PrimaryView::new(collection);
        let selected_request = Persistent::new(
            PersistentKey::RequestId,
            SelectedRequestId::default(),
        );
        Self {
            // State
            request_store: RequestStore::default(),
            selected_request,

            // Children
            primary_view: primary_view.into(),
            modal_queue: Component::default(),
            notification_text: None,
        }
    }

    /// Select the given request. This will ensure the request data is loaded
    /// in memory.
    fn select_request(
        &mut self,
        request_id: Option<RequestId>,
    ) -> anyhow::Result<()> {
        let primary_view = self.primary_view.data();

        let mut get_id = || -> anyhow::Result<Option<RequestId>> {
            if let Some(request_id) = request_id {
                // Make sure the given ID is valid, and the request is loaded.
                // This will return `None` if the ID wasn't in the DB, which
                // probably means we had a loading/failed request selected
                // before reload, so it's gone. Fall back to the top of the list
                if let Some(request_state) =
                    self.request_store.load(request_id)?
                {
                    return Ok(Some(request_state.id()));
                }
            }

            // We don't have a valid persisted ID, find the most recent for the
            // current recipe+profile
            if let Some(recipe_id) = primary_view.selected_recipe_id() {
                let profile_id = primary_view.selected_profile_id();
                Ok(self
                    .request_store
                    .load_latest(profile_id, recipe_id)?
                    .map(RequestState::id))
            } else {
                Ok(None)
            }
        };

        **self.selected_request = get_id()?;
        Ok(())
    }

    /// What request should be shown in the request/response pane right now?
    fn selected_request(&self) -> Option<&RequestState> {
        self.selected_request
            .and_then(|request_id| self.request_store.get(request_id))
    }

    /// Open the history modal for current recipe+profile. Return an error if
    /// the harness.database load failed.
    fn open_history(&mut self) -> anyhow::Result<()> {
        let primary_view = self.primary_view.data();
        if let Some(recipe) = primary_view.selected_recipe() {
            // Make sure all requests for this profile+recipe are loaded
            let requests = self
                .request_store
                .load_summaries(primary_view.selected_profile_id(), &recipe.id)?
                .map(RequestStateSummary::from)
                .collect();

            ViewContext::open_modal(
                History::new(recipe, requests, **self.selected_request),
                ModalPriority::Low,
            );
        }
        Ok(())
    }
}

impl EventHandler for Root {
    fn update(&mut self, event: Event) -> Update {
        match event {
            // Set selected request, and load it from the DB if needed
            Event::HttpSelectRequest(request_id) => {
                self.select_request(request_id)
                    .reported(&ViewContext::messages_tx());
            }
            // Update state of in-progress HTTP request
            Event::HttpSetState(state) => {
                let id = state.id();
                // If this request is *new*, select it
                if self.request_store.update(state) {
                    **self.selected_request = Some(id);
                }
            }

            Event::Notify(notification) => {
                self.notification_text =
                    Some(NotificationText::new(notification).into())
            }

            Event::Input {
                action: Some(action),
                ..
            } => match action {
                // Handle history here because we have stored request state
                Action::History => {
                    self.open_history().reported(&ViewContext::messages_tx());
                }
                Action::Quit => ViewContext::send_message(Message::Quit),
                Action::ReloadCollection => {
                    ViewContext::send_message(Message::CollectionStartReload)
                }
                _ => return Update::Propagate(event),
            },

            // Any other unhandled input event should *not* log an error,
            // because it is probably just unmapped input, and not a bug
            Event::Input { .. } => {}

            Event::Local(ref callback) => {
                match callback.downcast_ref::<GlobalAction>() {
                    Some(GlobalAction::EditCollection) => {
                        ViewContext::send_message(Message::CollectionEdit)
                    }
                    None => return Update::Propagate(event),
                }
            }

            // There shouldn't be anything left unhandled. Bubble up to log it
            _ => return Update::Propagate(event),
        }
        Update::Consumed
    }

    fn children(&mut self) -> Vec<Component<&mut dyn EventHandler>> {
        vec![self.modal_queue.as_child(), self.primary_view.as_child()]
    }
}

impl Draw for Root {
    fn draw(&self, frame: &mut Frame, _: (), metadata: DrawMetadata) {
        // Create layout
        let [main_area, footer_area] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(1)])
                .areas(metadata.area());

        // Main content
        self.primary_view.draw(
            frame,
            PrimaryViewProps {
                selected_request: self.selected_request(),
            },
            main_area,
            !self.modal_queue.data().is_open(),
        );

        // Footer
        let footer = HelpFooter.generate();
        let [notification_area, help_area] = Layout::horizontal([
            Constraint::Min(10),
            Constraint::Length(footer.width() as u16),
        ])
        .areas(footer_area);
        if let Some(notification_text) = &self.notification_text {
            notification_text.draw(frame, (), notification_area, false);
        }
        frame.render_widget(footer, help_area);

        // Render modals last so they go on top
        self.modal_queue.draw(frame, (), frame.size(), true);
    }
}

/// A wrapper for the selected request ID. This is needed to customize
/// persistence loading. We have to load the persisted value via an event so it
/// can be loaded from the DB.
#[derive(Debug, Default, Deref, DerefMut)]
struct SelectedRequestId(Option<RequestId>);

impl PersistentContainer for SelectedRequestId {
    type Value = RequestId;

    fn get(&self) -> Option<&Self::Value> {
        self.0.as_ref()
    }

    fn set(&mut self, value: <Self::Value as Persistable>::Persisted) {
        // We can't just set the value directly, because then the request won't
        // be loaded from the DB
        ViewContext::push_event(Event::HttpSelectRequest(Some(value)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        http::Exchange,
        test_util::{assert_matches, Factory},
        tui::{
            test_util::{harness, TestHarness},
            view::test_util::TestComponent,
        },
    };
    use crossterm::event::KeyCode;
    use rstest::rstest;

    /// Test that, on first render, the view loads the most recent historical
    /// request for the first recipe+profile
    #[rstest]
    fn test_preload_request(harness: TestHarness) {
        // Add a request into the DB that we expect to preload
        let collection = Collection::factory(());
        let profile_id = collection.first_profile_id();
        let recipe_id = collection.first_recipe_id();
        let exchange =
            Exchange::factory((Some(profile_id.clone()), recipe_id.clone()));
        harness.database.insert_exchange(&exchange).unwrap();

        let component = TestComponent::new(harness, Root::new(&collection), ());

        // Make sure profile+recipe were preselected correctly
        let primary_view = component.data().primary_view.data();
        assert_eq!(primary_view.selected_profile_id(), Some(profile_id));
        assert_eq!(primary_view.selected_recipe_id(), Some(recipe_id));
        assert_eq!(
            component.data().selected_request(),
            Some(&RequestState::Response { exchange })
        );

        // It'd be nice to assert on the view but it's just too complicated to
        // be worth mocking the whole thing out
    }

    /// Test that, on first render, if there's a persisted request ID, we load
    /// up to that instead of selecting the first in the list
    #[rstest]
    fn test_load_persisted_request(harness: TestHarness) {
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
        harness
            .database
            .set_ui(PersistentKey::RequestId, old_exchange.id)
            .unwrap();

        let component = TestComponent::new(harness, Root::new(&collection), ());

        // Make sure everything was preselected correctly
        assert_eq!(
            component.data().primary_view.data().selected_profile_id(),
            Some(profile_id)
        );
        assert_eq!(
            component.data().primary_view.data().selected_recipe_id(),
            Some(recipe_id)
        );
        assert_eq!(
            component.data().selected_request(),
            Some(&RequestState::Response {
                exchange: old_exchange
            })
        );
    }

    /// Test that if the persisted request ID isn't in the DB, we'll fall back
    /// to selecting the most recent request
    #[rstest]
    fn test_persisted_request_missing(harness: TestHarness) {
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
            .set_ui(PersistentKey::RequestId, RequestId::new())
            .unwrap();

        let component = TestComponent::new(harness, Root::new(&collection), ());

        assert_eq!(
            component.data().selected_request(),
            Some(&RequestState::Response {
                exchange: new_exchange
            })
        );
    }

    #[rstest]
    fn test_edit_collection(harness: TestHarness) {
        let collection = Collection::factory(());
        let mut component =
            TestComponent::new(harness, Root::new(&collection), ());

        component.harness_mut().clear_messages(); // Clear init junk

        // Event should be converted into a message appropriately
        // Open action menu
        component.send_key(KeyCode::Char('x')).assert_empty();
        // Select first action - Edit Collection
        component.send_key(KeyCode::Enter).assert_empty();
        assert_matches!(
            component.harness_mut().pop_message_now(),
            Message::CollectionEdit
        );
    }
}
