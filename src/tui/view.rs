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
            event::{Event, EventQueue, Update},
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
        self.root.draw(frame, (), chunk, true);
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
        // If we haven't done first render yet, don't drain the queue. This can
        // happen after a collection reload, because of the structure of the
        // main loop
        if !self.root.is_visible() {
            return;
        }

        // It's possible for components to queue additional events
        while let Some(event) = EventQueue::pop() {
            trace_span!("View event", ?event).in_scope(|| {
                match self.root.update_all(&self.messages_tx, event) {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        collection::Collection, test_util::*, tui::context::TuiContext,
    };
    use ratatui::{backend::TestBackend, Terminal};
    use rstest::rstest;

    /// Test view handling and drawing during initial view setup
    #[rstest]
    fn test_initial_draw(
        _tui_context: &TuiContext,
        mut terminal: Terminal<TestBackend>,
        messages: MessageQueue,
    ) {
        let collection = Collection::factory();
        let collection_file = CollectionFile::testing(collection);
        let mut view = View::new(&collection_file, messages.tx().clone());

        // Initial events
        assert_events!(
            Event::HttpLoadRequest,
            Event::Other(_),
            Event::Notify(_)
        );

        // Events should *still* be in the queue, because we haven't drawn yet
        view.handle_events();
        assert_events!(
            Event::HttpLoadRequest,
            Event::Other(_),
            Event::Notify(_)
        );

        // Nothing new
        view.draw(&mut terminal.get_frame());
        assert_events!(
            Event::HttpLoadRequest,
            Event::Other(_),
            Event::Notify(_)
        );

        // *Now* the queue is drained
        view.handle_events();
        assert_events!();
    }
}
