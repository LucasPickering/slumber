use crate::{
    collection::{Collection, ProfileId, RecipeId},
    tui::{
        input::Action,
        message::{Message, MessageSender},
        view::{
            common::{actions::GlobalAction, modal::ModalQueue},
            component::{
                help::HelpFooter,
                misc::NotificationText,
                primary::{PrimaryView, PrimaryViewProps},
            },
            draw::{Draw, DrawMetadata, Generate},
            event::{Event, EventHandler, Update},
            state::RequestState,
            Component,
        },
    },
};
use ratatui::{layout::Layout, prelude::Constraint, Frame};
use std::collections::{hash_map::Entry, HashMap};

/// The root view component
#[derive(derive_more::Debug)]
pub struct Root {
    // ===== Own State =====
    /// Cached request state. Request history is specific to both a recipe
    /// **and** a profile, so we must key on both. A profile+recipe pair will
    /// appear in this map if two conditions are met:
    /// - It has at least one *successful* request in history
    /// - It has beed focused by the user during this process
    /// This will be populated on-demand when a user selects a recipe in the
    /// list.
    #[debug(skip)]
    active_requests: HashMap<(Option<ProfileId>, RecipeId), RequestState>,

    // ==== Children =====
    /// We hold onto the primary view even when it's not visible, because we
    /// don't want the state to reset when changing views
    #[debug(skip)]
    primary_view: Component<PrimaryView>,
    #[debug(skip)]
    modal_queue: Component<ModalQueue>,
    #[debug(skip)]
    notification_text: Option<Component<NotificationText>>,
}

impl Root {
    pub fn new(collection: &Collection, messages_tx: MessageSender) -> Self {
        Self {
            // State
            active_requests: HashMap::new(),

            // Children
            primary_view: PrimaryView::new(collection, messages_tx).into(),
            modal_queue: Component::default(),
            notification_text: None,
        }
    }

    /// Get the request state to be displayed
    fn active_request(&self) -> Option<&RequestState> {
        let primary_view = self.primary_view.data();
        // "No Profile" _is_ a profile
        let profile_id = primary_view
            .selected_profile()
            .map(|profile| profile.id.clone());
        let recipe_id = primary_view.selected_recipe()?.id.clone();
        self.active_requests.get(&(profile_id, recipe_id))
    }

    /// Update the active HTTP request state
    fn update_request(
        &mut self,
        profile_id: Option<ProfileId>,
        recipe_id: RecipeId,
        state: RequestState,
    ) {
        // Update the state if any of these conditions match:
        // - There's nothing there yet
        // - This is a new request
        // - This is an update to the request already in place
        match self.active_requests.entry((profile_id, recipe_id)) {
            Entry::Vacant(entry) => {
                entry.insert(state);
            }
            Entry::Occupied(mut entry)
                if state.is_initial() || entry.get().id() == state.id() =>
            {
                entry.insert(state);
            }
            Entry::Occupied(_) => {
                // State is already holding a different request, throw
                // this update away
            }
        }
    }
}

impl EventHandler for Root {
    fn update(&mut self, messages_tx: &MessageSender, event: Event) -> Update {
        match event {
            // Update state of HTTP request
            Event::HttpSetState {
                profile_id,
                recipe_id,
                state,
            } => self.update_request(profile_id, recipe_id, state),

            Event::Notify(notification) => {
                self.notification_text =
                    Some(NotificationText::new(notification).into())
            }

            // Any input here should be handled regardless of current screen
            // context (barring any focused text element, which will eat all
            // input)
            Event::Input {
                action: Some(action),
                ..
            } => match action {
                Action::Quit => messages_tx.send(Message::Quit),
                Action::ReloadCollection => {
                    messages_tx.send(Message::CollectionStartReload)
                }
                _ => return Update::Propagate(event),
            },

            // Any other unhandled input event should *not* log an error,
            // because it is probably just unmapped input
            Event::Input { .. } => {}

            Event::Other(ref callback) => {
                match callback.downcast_ref::<GlobalAction>() {
                    Some(GlobalAction::EditCollection) => {
                        messages_tx.send(Message::CollectionEdit)
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
                active_request: self.active_request(),
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
