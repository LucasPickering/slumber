use crate::{
    config::{RequestCollection, RequestRecipeId},
    tui::{
        input::Action,
        message::Message,
        view::{
            component::{
                misc::{HelpText, HelpTextProps, NotificationText},
                modal::ModalQueue,
                primary::{PrimaryView, PrimaryViewProps},
                request::RequestPaneProps,
                response::ResponsePaneProps,
                Component, Draw, Event, Update, UpdateContext,
            },
            state::RequestState,
            util::layout,
            DrawContext,
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
    active_requests: HashMap<RequestRecipeId, RequestState>,
    fullscreen_mode: Option<FullscreenMode>,

    // ==== Children =====
    /// We hold onto the primary view even when it's not visible, because we
    /// don't want the state to reset when changing views
    primary_view: PrimaryView,
    // fullscreen_view: Option<FullscreenView>,
    modal_queue: ModalQueue,
    notification_text: Option<NotificationText>,
}

/// The various things that can be requested (haha get it, requested) to be
/// shown in fullscreen. If one of these is requested while not available, we
/// simply won't show it.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum FullscreenMode {
    /// Fullscreen the active request recipe
    Request,
    /// Fullscreen the active response
    Response,
}

impl Root {
    pub fn new(collection: &RequestCollection) -> Self {
        Self {
            // State
            active_requests: HashMap::new(),
            fullscreen_mode: None,

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
    fn update(&mut self, context: &mut UpdateContext, event: Event) -> Update {
        match event {
            Event::Init => {
                // Load the initial state for the selected recipe
                if let Some(recipe) = self.primary_view.selected_recipe() {
                    context.send_message(Message::RepositoryStartLoad {
                        recipe_id: recipe.id.clone(),
                    });
                }
            }

            // Update state of HTTP request
            Event::HttpSetState { recipe_id, state } => {
                self.update_request(recipe_id, state)
            }

            // Other state messages
            Event::ToggleFullscreen(mode) => {
                // If we're already in the given mode, exit
                self.fullscreen_mode = if Some(mode) == self.fullscreen_mode {
                    None
                } else {
                    Some(mode)
                };
            }
            Event::Notify(notification) => {
                self.notification_text =
                    Some(NotificationText::new(notification))
            }

            // Input messages
            Event::Input { action, .. } => match action {
                Some(Action::Quit) => context.send_message(Message::Quit),
                Some(Action::ReloadCollection) => {
                    context.send_message(Message::CollectionStartReload)
                }
                Some(Action::SendRequest) => {
                    // Send a request from anywhere
                    context.queue_event(Event::HttpSendRequest)
                }
                // Any other user input should get thrown away
                _ => {}
            },

            // There shouldn't be anything left unhandled. Bubble up to log it
            _ => return Update::Propagate(event),
        }
        Update::Consumed
    }

    fn children(&mut self) -> Vec<&mut dyn Component> {
        vec![
            &mut self.modal_queue,
            match self.fullscreen_mode {
                None => &mut self.primary_view,
                Some(FullscreenMode::Request) => {
                    self.primary_view.request_pane_mut()
                }
                Some(FullscreenMode::Response) => {
                    self.primary_view.response_pane_mut()
                }
            },
        ]
    }
}

impl Draw for Root {
    fn draw(&self, context: &mut DrawContext, _: (), chunk: Rect) {
        // Create layout
        let [main_chunk, footer_chunk] = layout(
            chunk,
            Direction::Vertical,
            [Constraint::Min(0), Constraint::Length(1)],
        );

        // Main content
        match self.fullscreen_mode {
            None => self.primary_view.draw(
                context,
                PrimaryViewProps {
                    active_request: self.active_request(),
                },
                main_chunk,
            ),
            Some(FullscreenMode::Request) => {
                self.primary_view.request_pane().draw(
                    context,
                    RequestPaneProps {
                        is_selected: false,
                        selected_recipe: self.primary_view.selected_recipe(),
                        selected_profile_id: self
                            .primary_view
                            .selected_profile()
                            .map(|profile| &profile.id),
                    },
                    main_chunk,
                );
            }
            Some(FullscreenMode::Response) => {
                self.primary_view.response_pane().draw(
                    context,
                    ResponsePaneProps {
                        is_selected: false,
                        active_request: self.active_request(),
                    },
                    main_chunk,
                );
            }
        }

        // Footer
        match &self.notification_text {
            Some(notification_text) => {
                notification_text.draw(context, (), footer_chunk)
            }
            None => HelpText.draw(
                context,
                HelpTextProps {
                    has_modal: self.modal_queue.is_open(),
                    fullscreen_mode: self.fullscreen_mode,
                    selected_pane: self.primary_view.selected_pane(),
                },
                footer_chunk,
            ),
        }

        // Render modals last so they go on top
        self.modal_queue.draw(context, (), context.frame.size());
    }
}
