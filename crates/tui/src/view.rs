mod common;
mod component;
mod context;
mod debug;
mod event;
mod state;
mod styles;
#[cfg(test)]
pub mod test_util;
mod util;

pub use component::ComponentMap;
pub use context::{UpdateContext, ViewContext};
pub use styles::Styles;
pub use util::{PreviewPrompter, Question, TuiPrompter};

use crate::{
    context::TuiContext,
    http::{RequestConfig, RequestState},
    input::InputEvent,
    message::MessageSender,
    util::PersistentStore,
    view::{
        component::{Canvas, Component, ComponentExt, Root},
        debug::DebugMonitor,
        event::Event,
    },
};
use crossterm::clipboard::CopyToClipboard;
use ratatui::{
    Frame,
    crossterm::execute,
    layout::{Constraint, Layout},
    text::{Span, Text},
};
use slumber_config::Action;
use slumber_core::{
    collection::{Collection, CollectionFile, ProfileId},
    database::CollectionDatabase,
};
use std::{
    fmt::{Debug, Display},
    io,
    sync::Arc,
};
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
    root: Root,
    /// Populated iff the `debug` config field is enabled. This tracks view
    /// metrics and displays them to the user.
    debug_monitor: Option<DebugMonitor>,
}

impl View {
    pub fn new(
        collection: &Arc<Collection>,
        database: CollectionDatabase,
        messages_tx: MessageSender,
    ) -> Self {
        ViewContext::init(Arc::clone(collection), database, messages_tx);

        let debug_monitor = if TuiContext::get().config.tui.debug {
            Some(DebugMonitor::default())
        } else {
            None
        };

        Self {
            root: Root::new(),
            debug_monitor,
        }
    }

    /// Draw the view to screen. This needs access to the input engine in order
    /// to render input bindings as help messages to the user.
    #[must_use]
    pub fn draw<'f>(&'f self, frame: &'f mut Frame) -> ComponentMap {
        // If the screen is too small to render anything, don't try. This avoids
        // panics within ratatui from trying to render borders and margins
        // outside the buffer area
        if frame.area().width <= 1 || frame.area().height <= 1 {
            return ComponentMap::default();
        }

        // If debug monitor is enabled, use it to capture the view duration
        if let Some(debug_monitor) = &self.debug_monitor {
            debug_monitor
                .draw(frame, |frame| Canvas::draw_all(frame, &self.root, ()))
        } else {
            Canvas::draw_all(frame, &self.root, ())
        }
    }

    /// When the collection fails to load on first launch, we can't show the
    /// full UI yet. This draws an error state. The TUI loop should be watching
    /// the collection file so we can retry initialization when the error is
    /// fixed.
    pub fn draw_collection_load_error(
        frame: &mut Frame,
        collection_file: &CollectionFile,
        error: &anyhow::Error,
    ) {
        let context = TuiContext::get();
        let [message_area, _, error_area] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Length(1), // A nice gap
            Constraint::Min(1),
        ])
        .areas(frame.area());
        frame.render_widget(error.generate(), error_area);
        frame.render_widget(
            Text::styled(
                format!(
                    "Watching {collection_file} for changes...\n{} to exit",
                    context.input_engine.binding_display(Action::ForceQuit),
                ),
                context.styles.text.primary,
            ),
            message_area,
        );
    }

    /// Persist all UI state to the database. This should be called at the end
    /// of each update phase. It does *not* need to be called after each
    /// individual event when multiple events are batched together.
    ///
    /// This takes `&mut self` because we dynamically load children, and those
    /// are always mutable.
    pub fn persist(&mut self, database: &CollectionDatabase) {
        self.root.persist_all(&mut PersistentStore::new(database));
    }

    /// ID of the selected profile. `None` iff the list is empty
    pub fn selected_profile_id(&self) -> Option<&ProfileId> {
        self.root.selected_profile_id()
    }

    /// Get a definition of the request that should be sent from the current
    /// recipe settings
    pub fn request_config(&self) -> Option<RequestConfig> {
        self.root.request_config()
    }

    /// Update the UI to reflect the current state of an HTTP request. If
    /// `select` is `true`, select the request too.
    pub fn update_request(&mut self, state: &RequestState, select: bool) {
        self.root.update_request(state, select);
    }

    /// Ask the user a [Question]
    pub fn question(&mut self, question: Question) {
        self.root.question(question);
    }

    /// Display an error to the user in a modal
    pub fn error(&mut self, error: anyhow::Error) {
        self.root.error(error);
    }

    /// Display an informational notification to the user
    pub fn notify(&mut self, message: impl ToString) {
        self.root.notify(message.to_string());
    }

    /// Queue an event to update the view according to an input event from the
    /// user. If possible, a bound action is provided which tells us what
    /// abstract action the input maps to.
    pub fn handle_input(&self, event: InputEvent) {
        ViewContext::push_event(Event::Input(event));
    }

    /// Drain all view events from the queue. The component three will process
    /// events one by one. This should be called on every TUI loop. Return
    /// whether or not an event was handled.
    pub fn handle_events(&mut self, mut context: UpdateContext) -> bool {
        // If we haven't done first render yet, don't drain the queue. This can
        // happen after a collection reload, because of the structure of the
        // main loop
        if !context.component_map.is_visible(&self.root) {
            return false;
        }

        let mut handled = false;
        // It's possible for components to queue additional events, so keep
        // going until the queue is empty
        while let Some(event) = ViewContext::pop_event() {
            handled = true;
            trace_span!("Handling event", ?event).in_scope(|| {
                match self.root.update_all(&mut context, event) {
                    None => trace!("Event consumed"),
                    // Consumer didn't eat the event - huh?
                    Some(event) => warn!(?event, "Event was unhandled"),
                }
            });
        }
        handled
    }

    /// Copy text to the user's clipboard, and notify them
    pub fn copy_text(&mut self, text: String) -> anyhow::Result<()> {
        execute!(io::stdout(), CopyToClipboard::to_clipboard_from(text))
            .inspect(|()| self.notify("Copied text to clipboard"))
            .map_err(|error| {
                anyhow::Error::from(error).context("Error copying text")
            })
    }
}

/// A helper for building a UI. It can be converted into some UI element to be
/// drawn.
pub trait Generate {
    type Output<'this>
    where
        Self: 'this;

    /// Build a UI element
    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this;
}

/// Marker trait to pull in a blanket impl of [Generate], which simply calls
/// [ToString::to_string] on the value to create a [ratatui::text::Span].
pub trait ToStringGenerate: Display {}

impl ToStringGenerate for &str {}

impl<T> Generate for &T
where
    T: ToStringGenerate,
{
    type Output<'this>
        = Span<'this>
    where
        Self: 'this;

    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this,
    {
        self.to_string().into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{
        TestHarness, TestTerminal, assert_events, harness, terminal,
    };
    use rstest::rstest;
    use slumber_core::collection::Collection;
    use slumber_util::Factory;

    /// Test view handling and drawing during initial view setup
    #[rstest]
    fn test_initial_draw(harness: TestHarness, terminal: TestTerminal) {
        let collection = Collection::factory(());
        let mut view = View::new(
            &collection.into(),
            harness.database.clone(),
            harness.messages_tx().clone(),
        );

        // Initial events
        assert_events!(
            Event::Emitted { .. }, // Recipe list selection
            Event::Emitted { .. }, // Primary pane selection
        );

        // Events should *still* be in the queue, because we haven't drawn yet
        let mut component_map = ComponentMap::default();
        let mut request_store = harness.request_store.borrow_mut();
        view.handle_events(UpdateContext {
            component_map: &component_map,
            request_store: &mut request_store,
        });
        assert_events!(Event::Emitted { .. }, Event::Emitted { .. },);

        // Nothing new
        terminal.draw(|frame| component_map = view.draw(frame));
        assert_events!(Event::Emitted { .. }, Event::Emitted { .. },);

        // *Now* the queue is drained
        view.handle_events(UpdateContext {
            component_map: &component_map,
            request_store: &mut request_store,
        });
        assert_events!();
    }
}
