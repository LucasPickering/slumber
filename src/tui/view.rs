mod common;
mod component;
mod draw;
mod event;
mod state;
mod theme;
mod util;

pub use common::modal::{IntoModal, ModalPriority};
pub use state::RequestState;
pub use theme::Theme;
pub use util::PreviewPrompter;

use crate::{
    collection::{Collection, ProfileId, RecipeId},
    tui::{
        context::TuiContext,
        input::Action,
        message::Message,
        view::{
            component::{Component, Root},
            draw::Draw,
            event::{Event, EventHandler, Update, UpdateContext},
            state::Notification,
        },
    },
};
use anyhow::anyhow;
use ratatui::Frame;
use std::{collections::VecDeque, fmt::Debug};
use tracing::{error, trace, trace_span};

/// Primary entrypoint for the view. This contains the main draw functions, as
/// well as bindings for externally modifying the view state. We use a component
/// architecture based on React, meaning the view is responsible for managing
/// its own state. Certain global state (e.g. the database) is managed by the
/// controller and exposed via event passing.
#[derive(Debug)]
pub struct View {
    root: Component<Root>,
}

impl View {
    pub fn new(collection: &Collection) -> Self {
        let mut view = Self {
            root: Root::new(collection).into(),
        };
        // Tell the components to wake up
        view.handle_event(Event::Init);
        view
    }

    /// Draw the view to screen. This needs access to the input engine in order
    /// to render input bindings as help messages to the user.
    pub fn draw<'a>(&'a self, frame: &'a mut Frame) {
        let chunk = frame.size();
        self.root.draw(frame, (), chunk)
    }

    /// Update the request state for the given profile+recipe. The state will
    /// only be updated if this is a new request or it matches the current
    /// request for this recipe. We only store one request per profile+recipe at
    /// a time.
    pub fn set_request_state(
        &mut self,
        profile_id: Option<ProfileId>,
        recipe_id: RecipeId,
        state: RequestState,
    ) {
        self.handle_event(Event::HttpSetState {
            profile_id,
            recipe_id,
            state,
        });
    }

    /// Open a new modal. The input can be anything that converts to modal
    /// content
    pub fn open_modal(
        &mut self,
        modal: impl IntoModal + 'static,
        priority: ModalPriority,
    ) {
        self.handle_event(Event::OpenModal {
            modal: Box::new(modal.into_modal()),
            priority,
        });
    }

    /// Send an informational notification to the user
    pub fn notify(&mut self, message: impl ToString) {
        let notification = Notification::new(message.to_string());
        self.handle_event(Event::Notify(notification));
    }

    /// Copy text to the user's clipboard, and notify them
    pub fn copy_text(&mut self, text: String) {
        match cli_clipboard::set_contents(text) {
            Ok(()) => {
                self.notify("Copied text to clipboard");
            }
            Err(error) => {
                // Returned error doesn't impl 'static so we can't
                // directly convert it to anyhow
                TuiContext::send_message(Message::Error {
                    error: anyhow!("Error copying text: {error}"),
                })
            }
        }
    }

    /// Update the view according to an input event from the user. If possible,
    /// a bound action is provided which tells us what abstract action the
    /// input maps to.
    pub fn handle_input(
        &mut self,
        event: crossterm::event::Event,
        action: Option<Action>,
    ) {
        self.handle_event(Event::Input { event, action })
    }

    /// Process a view event by passing it to the root component and letting
    /// it pass it down the tree
    fn handle_event(&mut self, event: Event) {
        let mut event_queue: VecDeque<Event> = [event].into();

        // Each event being handled could potentially queue more. Keep going
        // until the queue is drained
        while let Some(event) = event_queue.pop_front() {
            // Certain events *just don't matter*, AT ALL. They're not even
            // supposed to be around, like, in the area
            if event.should_kill() {
                continue;
            }

            let span = trace_span!("View event", ?event);
            span.in_scope(|| {
                let mut context = UpdateContext::new(&mut event_queue);

                let update =
                    Self::update_all(self.root.as_child(), &mut context, event);
                match update {
                    Update::Consumed => {
                        trace!("View event consumed")
                    }
                    // Consumer didn't eat the event - huh?
                    Update::Propagate(_) => {
                        error!("View event was unhandled");
                    }
                }
            });
        }
    }

    /// Update the state of a component *and* its children, starting at the
    /// lowest descendant. Recursively walk up the tree until a component
    /// consumes the event.
    fn update_all(
        mut component: Component<&mut dyn EventHandler>,
        context: &mut UpdateContext,
        mut event: Event,
    ) -> Update {
        // If we have a child, send them the event. If not, eat it ourselves
        for child in component.children() {
            if event.should_handle(&child) {
                let update = Self::update_all(child, context, event); // RECURSION
                match update {
                    Update::Propagate(returned) => {
                        // Keep going to the next child. It's possible the child
                        // returned something other than the original event,
                        // which we'll just pass along
                        // anyway.
                        event = returned;
                    }
                    Update::Consumed => {
                        return update;
                    }
                }
            }
        }

        // None of our children handled it, we'll take it ourselves.
        // Message is already traced in the parent span, so don't dupe it.
        let span = trace_span!(
            "Component handling",
            component = ?component.inner(),
        );
        span.in_scope(|| {
            let update = component.update(context, event);
            trace!(?update);
            update
        })
    }
}
