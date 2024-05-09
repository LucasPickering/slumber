mod common;
mod component;
mod draw;
mod event;
mod state;
mod theme;
mod util;

pub use common::modal::{IntoModal, ModalPriority};
pub use state::RequestState;
pub use theme::{Styles, Theme};
pub use util::{Confirm, PreviewPrompter};

use crate::{
    collection::{CollectionFile, ProfileId, RecipeId},
    tui::{
        input::Action,
        message::{Message, MessageSender},
        view::{
            component::{Component, Root},
            draw::Draw,
            event::{Event, EventHandler, EventQueue, Update},
            state::Notification,
        },
    },
};
use anyhow::anyhow;
use ratatui::Frame;
use std::fmt::Debug;
use tracing::{error, trace, trace_span};

/// Primary entrypoint for the view. This contains the main draw functions, as
/// well as bindings for externally modifying the view state. We use a component
/// architecture based on React, meaning the view is responsible for managing
/// its own state. Certain global state (e.g. the database) is managed by the
/// controller and exposed via event passing.
///
/// External updates on the view are lazy, meaning calls to methods like
/// [Self::handle_input] simply queue an event to handle the input. Call
/// [Self::handle_events] to drain the queue once per loop. This is necessary
/// because events can be triggered from other places too (e.g. from other
/// events), so we need to make sure the queue is constantly being drained.
#[derive(Debug)]
pub struct View {
    root: Component<Root>,
    /// A channel for sending async messages to the main loop
    messages_tx: MessageSender,
}

impl View {
    pub fn new(
        collection_file: &CollectionFile,
        messages_tx: MessageSender,
    ) -> Self {
        let mut view = Self {
            // Forward the message sender so it can be used during component
            // construction
            root: Root::new(&collection_file.collection, messages_tx.clone())
                .into(),
            messages_tx,
        };
        view.notify(format!(
            "Loaded collection from {}",
            collection_file.path().to_string_lossy()
        ));
        view
    }

    /// Draw the view to screen. This needs access to the input engine in order
    /// to render input bindings as help messages to the user.
    pub fn draw<'a>(&'a self, frame: &'a mut Frame) {
        let chunk = frame.size();
        self.root.draw(frame, (), chunk)
    }

    /// Queue an event to update the request state for the given profile+recipe.
    /// The state will only be updated if this is a new request or it
    /// matches the current request for this recipe. We only store one
    /// request per profile+recipe at a time.
    pub fn set_request_state(
        &mut self,
        profile_id: Option<ProfileId>,
        recipe_id: RecipeId,
        state: RequestState,
    ) {
        EventQueue::push(Event::HttpSetState {
            profile_id,
            recipe_id,
            state,
        });
    }

    /// Queue an event to open a new modal. The input can be anything that
    /// converts to modal content
    pub fn open_modal(
        &mut self,
        modal: impl IntoModal + 'static,
        priority: ModalPriority,
    ) {
        EventQueue::push(Event::OpenModal {
            modal: Box::new(modal.into_modal()),
            priority,
        });
    }

    /// Queue an event to send an informational notification to the user
    pub fn notify(&mut self, message: impl ToString) {
        let notification = Notification::new(message.to_string());
        EventQueue::push(Event::Notify(notification));
    }

    /// Queue an event to update the view according to an input event from the
    /// user. If possible, a bound action is provided which tells us what
    /// abstract action the input maps to.
    pub fn handle_input(
        &mut self,
        event: crossterm::event::Event,
        action: Option<Action>,
    ) {
        EventQueue::push(Event::Input { event, action })
    }

    /// Drain all view events from the queue. The component three will process
    /// events one by one. This should be called on every TUI loop
    pub fn handle_events(&mut self) {
        // It's possible for components to queue additional events
        while let Some(event) = EventQueue::pop() {
            trace_span!("View event", ?event).in_scope(|| {
                match Self::update_all(
                    &self.messages_tx,
                    self.root.as_child(),
                    event,
                ) {
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

    /// Copy text to the user's clipboard, and notify them
    pub fn copy_text(&mut self, text: String) {
        match cli_clipboard::set_contents(text) {
            Ok(()) => self.notify("Copied text to clipboard"),
            Err(error) => {
                // Returned error doesn't impl 'static so we can't
                // directly convert it to anyhow
                self.messages_tx.send(Message::Error {
                    error: anyhow!("Error copying text: {error}"),
                })
            }
        }
    }

    /// Update the state of a component *and* its children, starting at the
    /// lowest descendant. Recursively walk up the tree until a component
    /// consumes the event.
    fn update_all(
        messages_tx: &MessageSender,
        mut component: Component<&mut dyn EventHandler>,
        mut event: Event,
    ) -> Update {
        // If we have a child, send them the event. If not, eat it ourselves
        for child in component.data_mut().children() {
            if event.should_handle(&child) {
                // RECURSION
                let update = Self::update_all(messages_tx, child, event);
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
            component = ?component.data(),
        );
        span.in_scope(|| {
            let update = component.data_mut().update(messages_tx, event);
            trace!(?update);
            update
        })
    }
}
