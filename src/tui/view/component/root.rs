use crate::{
    config::{RequestCollection, RequestRecipeId},
    tui::{
        input::Action,
        message::Message,
        view::{
            component::{
                misc::{HelpText, NotificationText},
                modal::ModalQueue,
                primary::{PrimaryView, PrimaryViewProps},
                response::ResponsePaneProps,
                Component, Draw, UpdateOutcome, ViewMessage,
            },
            state::RequestState,
            util::layout,
            Frame, RenderContext,
        },
    },
};
use derive_more::Display;
use ratatui::prelude::{Constraint, Direction, Rect};
use std::collections::{hash_map::Entry, HashMap};

/// The root view component
#[derive(Debug, Display)]
#[display(fmt = "Root")]
pub struct Root {
    // ===== Own State =====
    /// Cached request state. A recipe will appear in this map if two
    /// conditions are met:
    /// - It has at least one *successful* request in history
    /// - It has beed focused by the user during this process
    /// This will be populated on-demand when a user selects a recipe in the
    /// list.
    #[display(fmt = "")]
    active_requests: HashMap<RequestRecipeId, RequestState>,
    /// What is we lookin at?
    mode: RootMode,

    // ==== Children =====
    /// We hold onto the primary view even when it's not visible, because we
    /// don't want the state to reset when changing views
    primary_view: PrimaryView,
    modal_queue: ModalQueue,
    notification_text: Option<NotificationText>,
}

/// View mode of the root component
#[derive(Copy, Clone, Debug, Default)]
pub enum RootMode {
    /// Show the normal pane view
    #[default]
    Primary,
    /// Fullscreen the active response
    Response,
}

impl Root {
    pub fn new(collection: &RequestCollection) -> Self {
        Self {
            // State
            active_requests: HashMap::new(),
            mode: RootMode::default(),

            // Children
            primary_view: PrimaryView::new(collection),
            modal_queue: ModalQueue::new(),
            notification_text: None,
        }
    }

    /// Get the request state to be displayed
    fn active_request(&self) -> Option<&RequestState> {
        let recipe = self.primary_view.selected_recipe()?;
        self.active_requests.get(&recipe.id)
    }

    /// Update the active HTTP request state
    fn update_request(
        &mut self,
        recipe_id: RequestRecipeId,
        state: RequestState,
    ) {
        // Update the state if any of these conditions match:
        // - There's nothing there yet
        // - This is a new request
        // - This is an update to the request already in place
        match self.active_requests.entry(recipe_id) {
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

impl Component for Root {
    fn update(&mut self, message: ViewMessage) -> UpdateOutcome {
        match message {
            ViewMessage::Init => {
                // Load the initial state for the selected recipe
                tracing::trace!(recipe=?self.primary_view.selected_recipe(), "asdfasdfasdf");
                if let Some(recipe) = self.primary_view.selected_recipe() {
                    UpdateOutcome::SideEffect(Message::RepositoryStartLoad {
                        recipe_id: recipe.id.clone(),
                    })
                } else {
                    UpdateOutcome::Consumed
                }
            }

            // Update state of HTTP request
            ViewMessage::HttpSetState { recipe_id, state } => {
                self.update_request(recipe_id, state);
                UpdateOutcome::Consumed
            }

            // Other state messages
            ViewMessage::OpenView(mode) => {
                self.mode = mode;
                UpdateOutcome::Consumed
            }
            ViewMessage::Notify(notification) => {
                self.notification_text =
                    Some(NotificationText::new(notification));
                UpdateOutcome::Consumed
            }

            // Input messages
            ViewMessage::Input {
                action: Some(Action::Quit),
                ..
            } => UpdateOutcome::SideEffect(Message::Quit),
            ViewMessage::Input {
                action: Some(Action::ReloadCollection),
                ..
            } => UpdateOutcome::SideEffect(Message::CollectionStartReload),
            // Any other user input should get thrown away
            ViewMessage::Input { .. } => UpdateOutcome::Consumed,

            // There shouldn't be anything left unhandled. Bubble up to log it
            _ => UpdateOutcome::Propagate(message),
        }
    }

    fn children(&mut self) -> Vec<&mut dyn Component> {
        vec![
            &mut self.modal_queue,
            match self.mode {
                RootMode::Primary => &mut self.primary_view,
                RootMode::Response => self.primary_view.response_pane_mut(),
            },
        ]
    }
}

impl Draw for Root {
    fn draw(
        &self,
        context: &RenderContext,
        _: (),
        frame: &mut Frame,
        chunk: Rect,
    ) {
        // Create layout
        let [main_chunk, footer_chunk] = layout(
            chunk,
            Direction::Vertical,
            [Constraint::Min(0), Constraint::Length(1)],
        );

        // Main content
        match self.mode {
            RootMode::Primary => self.primary_view.draw(
                context,
                PrimaryViewProps {
                    active_request: self.active_request(),
                },
                frame,
                main_chunk,
            ),
            RootMode::Response => self.primary_view.response_pane().draw(
                context,
                ResponsePaneProps {
                    active_request: self.active_request(),
                    is_selected: false,
                },
                frame,
                main_chunk,
            ),
        }

        // Footer
        match &self.notification_text {
            Some(notification_text) => {
                notification_text.draw(context, (), frame, footer_chunk)
            }
            None => HelpText.draw(context, (), frame, footer_chunk),
        }

        // Render modals last so they go on top
        self.modal_queue.draw(context, (), frame, frame.size());
    }
}
