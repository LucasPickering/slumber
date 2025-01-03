mod common;
mod component;
mod context;
mod debug;
mod draw;
mod event;
mod state;
mod styles;
#[cfg(test)]
pub mod test_util;
mod util;

pub use common::modal::{IntoModal, ModalPriority};
pub use context::{UpdateContext, ViewContext};
pub use styles::Styles;
pub use util::{Confirm, PreviewPrompter};

use crate::{
    context::TuiContext,
    http::{RequestState, RequestStore},
    message::{Message, MessageSender},
    util::ResultReported,
    view::{
        component::{Component, Root},
        debug::DebugMonitor,
        event::Event,
    },
};
use anyhow::anyhow;
use ratatui::Frame;
use slumber_config::Action;
use slumber_core::{
    collection::{CollectionFile, ProfileId},
    db::CollectionDatabase,
    http::RequestId,
};
use std::{fmt::Debug, sync::Arc};
use tracing::{trace, trace_span, warn};

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
    /// Populated iff the `debug` config field is enabled. This tracks view
    /// metrics and displays them to the user.
    debug_monitor: Option<DebugMonitor>,
}

impl View {
    pub fn new(
        collection_file: &CollectionFile,
        request_store: &RequestStore,
        database: CollectionDatabase,
        messages_tx: MessageSender,
    ) -> Self {
        ViewContext::init(
            Arc::clone(&collection_file.collection),
            database,
            messages_tx,
        );

        let debug_monitor = if TuiContext::get().config.debug {
            Some(DebugMonitor::default())
        } else {
            None
        };

        let mut view = Self {
            root: Root::new(&collection_file.collection, request_store).into(),
            debug_monitor,
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
        fn draw_impl(root: &Component<Root>, frame: &mut Frame) {
            let chunk = frame.area();
            root.draw(frame, (), chunk, true);
        }

        // If debug monitor is enabled, use it to capture the view duration
        if let Some(debug_monitor) = &self.debug_monitor {
            debug_monitor.draw(frame, |frame| draw_impl(&self.root, frame));
        } else {
            draw_impl(&self.root, frame);
        }
    }

    /// ID of the selected profile. `None` iff the list is empty
    pub fn selected_profile_id(&self) -> Option<&ProfileId> {
        self.root.data().selected_profile_id()
    }

    /// Select a particular request
    pub fn select_request(
        &mut self,
        request_store: &mut RequestStore,
        request_id: RequestId,
    ) {
        self.root
            .data_mut()
            .select_request(request_store, Some(request_id))
            .reported(&ViewContext::messages_tx());
    }

    /// Notify the view that a request's state has changed in the store. If the
    /// request is selected, view state will be updated accordingly
    pub fn update_request(&mut self, request_state: &RequestState) {
        self.root.data_mut().update_request(request_state);
    }

    /// Queue an event to open a new modal. The input can be anything that
    /// converts to modal content
    pub fn open_modal(&mut self, modal: impl IntoModal + 'static) {
        ViewContext::open_modal(modal.into_modal());
    }

    /// Queue an event to send an informational notification to the user
    pub fn notify(&mut self, message: impl ToString) {
        ViewContext::notify(message);
    }

    /// Queue an event to update the view according to an input event from the
    /// user. If possible, a bound action is provided which tells us what
    /// abstract action the input maps to.
    pub fn handle_input(
        &mut self,
        event: crossterm::event::Event,
        action: Option<Action>,
    ) {
        ViewContext::push_event(Event::Input { event, action })
    }

    /// Drain all view events from the queue. The component three will process
    /// events one by one. This should be called on every TUI loop. Return
    /// whether or not an event was handled.
    pub fn handle_events(&mut self, mut context: UpdateContext) -> bool {
        // If we haven't done first render yet, don't drain the queue. This can
        // happen after a collection reload, because of the structure of the
        // main loop
        if !self.root.is_visible() {
            return false;
        }

        let mut handled = false;
        // It's possible for components to queue additional events, so keep
        // going until the queue is empty
        while let Some(event) = ViewContext::pop_event() {
            handled = true;
            trace_span!("View event", ?event).in_scope(|| {
                match self.root.update_all(&mut context, event) {
                    None => trace!("View event consumed"),
                    // Consumer didn't eat the event - huh?
                    Some(event) => warn!(?event, "View event was unhandled"),
                }
            });
        }
        handled
    }

    /// Copy text to the user's clipboard, and notify them
    pub fn copy_text(&mut self, text: String) {
        match cli_clipboard::set_contents(text) {
            Ok(()) => self.notify("Copied text to clipboard"),
            Err(error) => {
                // Returned error doesn't impl 'static so we can't
                // directly convert it to anyhow
                ViewContext::send_message(Message::Error {
                    error: anyhow!("Error copying text: {error}"),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{
        assert_events, harness, terminal, TestHarness, TestTerminal,
    };
    use rstest::rstest;
    use slumber_core::{collection::Collection, test_util::Factory};

    /// Test view handling and drawing during initial view setup
    #[rstest]
    fn test_initial_draw(harness: TestHarness, terminal: TestTerminal) {
        let collection = Collection::factory(());
        let collection_file = CollectionFile::factory(collection);
        let mut view = View::new(
            &collection_file,
            &harness.request_store.borrow(),
            harness.database.clone(),
            harness.messages_tx().clone(),
        );

        // Initial events
        assert_events!(
            Event::Emitted { .. }, // Recipe list selection
            Event::Emitted { .. }, // Primary pane selection
            Event::Notify(_),
        );

        // Events should *still* be in the queue, because we haven't drawn yet
        let mut request_store = harness.request_store.borrow_mut();
        view.handle_events(UpdateContext {
            request_store: &mut request_store,
        });
        assert_events!(
            Event::Emitted { .. },
            Event::Emitted { .. },
            Event::Notify(_),
        );

        // Nothing new
        terminal.draw(|frame| view.draw(frame));
        assert_events!(
            Event::Emitted { .. },
            Event::Emitted { .. },
            Event::Notify(_),
        );

        // *Now* the queue is drained
        view.handle_events(UpdateContext {
            request_store: &mut request_store,
        });
        assert_events!();
    }
}
