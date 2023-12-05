use crate::{
    collection::{RequestCollection, RequestRecipeId},
    tui::{
        context::TuiContext,
        input::Action,
        message::Message,
        view::{
            common::modal::ModalQueue,
            component::{
                help::HelpFooter,
                misc::NotificationText,
                primary::{PrimaryView, PrimaryViewProps},
            },
            draw::{Draw, DrawContext},
            event::{Event, EventHandler, Update, UpdateContext},
            state::RequestState,
            util::layout,
            Component,
        },
    },
};
use ratatui::prelude::{Constraint, Direction, Rect};
use std::{
    collections::{hash_map::Entry, HashMap},
    rc::Rc,
};

/// The root view component
#[derive(derive_more::Debug)]
pub struct Root {
    // ===== Own State =====
    /// Cached request state. A recipe will appear in this map if two
    /// conditions are met:
    /// - It has at least one *successful* request in history
    /// - It has beed focused by the user during this process
    /// This will be populated on-demand when a user selects a recipe in the
    /// list.
    #[debug(skip)]
    active_requests: HashMap<RequestRecipeId, RequestState>,

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
    pub fn new(collection: Rc<RequestCollection>) -> Self {
        Self {
            // State
            active_requests: HashMap::new(),

            // Children
            primary_view: PrimaryView::new(collection).into(),
            modal_queue: Component::default(),
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

impl EventHandler for Root {
    fn update(&mut self, _context: &mut UpdateContext, event: Event) -> Update {
        match event {
            Event::Init => {
                // Load the initial state for the selected recipe
                if let Some(recipe) = self.primary_view.selected_recipe() {
                    TuiContext::send_message(Message::RequestLoad {
                        recipe_id: recipe.id.clone(),
                    });
                }
            }

            // Update state of HTTP request
            Event::HttpSetState { recipe_id, state } => {
                self.update_request(recipe_id, state)
            }

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
                Action::Quit => TuiContext::send_message(Message::Quit),
                Action::ReloadCollection => {
                    TuiContext::send_message(Message::CollectionStartReload)
                }
                _ => return Update::Propagate(event),
            },

            // Any other unhandled input event should *not* log an error,
            // because it is probably just unmapped input
            Event::Input { .. } => {}

            // There shouldn't be anything left unhandled. Bubble up to log it
            _ => return Update::Propagate(event),
        }
        Update::Consumed
    }

    fn children(&mut self) -> Vec<Component<&mut dyn EventHandler>> {
        let modal_open = self.modal_queue.is_open();
        let mut children: Vec<Component<&mut dyn EventHandler>> =
            vec![self.modal_queue.as_child()];

        // If a modal is open, don't allow *any* input to the background. We'll
        // still accept input ourselves though, which should only be
        // high-priority stuff
        if !modal_open {
            children.push(self.primary_view.as_child());
        }

        children
    }
}

impl Draw for Root {
    fn draw(&self, context: &mut DrawContext, _: (), area: Rect) {
        // Create layout
        let [main_area, footer_area] = layout(
            area,
            Direction::Vertical,
            [Constraint::Min(0), Constraint::Length(1)],
        );

        // Main content
        self.primary_view.draw(
            context,
            PrimaryViewProps {
                active_request: self.active_request(),
            },
            main_area,
        );

        // Footer
        let [notification_area, help_area] = layout(
            footer_area,
            Direction::Horizontal,
            [Constraint::Min(10), Constraint::Length(29)],
        );
        if let Some(notification_text) = &self.notification_text {
            notification_text.draw(context, (), notification_area);
        }
        HelpFooter.draw(context, (), help_area);

        // Render modals last so they go on top
        self.modal_queue.draw(context, (), context.frame.size());
    }
}
