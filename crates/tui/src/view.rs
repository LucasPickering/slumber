mod common;
mod component;
mod context;
mod debug;
mod event;
pub mod persistent;
mod state;
mod styles;
#[cfg(test)]
mod test_util;
mod util;

pub use component::ComponentMap;
pub use context::UpdateContext;
pub use styles::Styles;
pub use util::{PreviewPrompter, Question, TuiPrompter};

use crate::{
    context::TuiContext,
    http::{RequestConfig, RequestState, RequestStore},
    input::InputEvent,
    message::MessageSender,
    view::{
        component::{Canvas, Component, ComponentExt, Root},
        context::ViewContext,
        debug::DebugMonitor,
        event::Event,
    },
};
use crossterm::clipboard::CopyToClipboard;
use indexmap::IndexMap;
use ratatui::{
    buffer::Buffer,
    crossterm::execute,
    layout::{Constraint, Layout},
    text::{Span, Text},
    widgets::Widget,
};
use slumber_config::Action;
use slumber_core::{
    collection::{Collection, CollectionError, CollectionFile, ProfileId},
    database::CollectionDatabase,
    http::RequestId,
};
use slumber_template::Template;
use std::{
    error::Error as StdError,
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
    /// Root component if the collection is valid. If not, this will be the
    /// error that prevented the collection from loading.
    root: Result<Root, InvalidCollection>,
    /// Populated iff the `debug` config field is enabled. This tracks view
    /// metrics and displays them to the user.
    debug_monitor: Option<DebugMonitor>,
}

impl View {
    /// Initialize the view. This will build out the entire component tree
    ///
    /// This accepts a loaded collection *or* an error. If the collection fails
    /// to load, we'll show the error and wait for the user to fix it or exit.
    pub fn new(
        collection: Result<Arc<Collection>, InvalidCollection>,
        database: CollectionDatabase,
        messages_tx: MessageSender,
    ) -> Self {
        let root = match collection {
            Ok(collection) => {
                ViewContext::init(collection, database, messages_tx);
                Ok(Root::new())
            }
            Err(error) => {
                // Put a placeholder collection in the context. This is a bit
                // of a hack, but ensures the context is populated for other
                // things that need it. We won't render any components that
                // rely on the collection.
                ViewContext::init(Default::default(), database, messages_tx);
                Err(error)
            }
        };

        let debug_monitor = if TuiContext::get().config.tui.debug {
            Some(DebugMonitor::default())
        } else {
            None
        };

        Self {
            root,
            debug_monitor,
        }
    }

    /// Draw the view to a screen buffer
    ///
    /// Return the map of all drawn components.
    #[must_use]
    pub fn draw<'f>(&'f self, buffer: &'f mut Buffer) -> ComponentMap {
        // If the screen is too small to render anything, don't try. This avoids
        // panics within ratatui from trying to render borders and margins
        // outside the buffer area
        if buffer.area().width <= 1 || buffer.area().height <= 1 {
            return ComponentMap::default();
        }

        match &self.root {
            Ok(root) => {
                // If debug monitor is enabled, use it to capture view duration
                if let Some(debug_monitor) = &self.debug_monitor {
                    debug_monitor.draw(buffer, |buffer| {
                        Canvas::draw_all(buffer, root, ())
                    })
                } else {
                    Canvas::draw_all(buffer, root, ())
                }
            }
            Err(invalid) => {
                let context = TuiContext::get();
                let [message_area, _, error_area] = Layout::vertical([
                    Constraint::Length(2),
                    Constraint::Length(1), // A nice gap
                    Constraint::Min(1),
                ])
                .areas(*buffer.area());
                Widget::render(
                    (&*invalid.error as &dyn StdError).generate(),
                    error_area,
                    buffer,
                );
                Widget::render(
                    Text::styled(
                        format!(
                            "Watching {file} for changes...\n{key} to exit",
                            file = invalid.file,
                            key = context
                                .input_engine
                                .binding_display(Action::ForceQuit),
                        ),
                        context.styles.text.primary,
                    ),
                    message_area,
                    buffer,
                );
                ComponentMap::default()
            }
        }
    }

    /// Persist all UI state to the database. This should be called at the end
    /// of each update phase. It does *not* need to be called after each
    /// individual event when multiple events are batched together.
    ///
    /// This takes `&mut self` because we dynamically load children, and those
    /// are always mutable.
    pub fn persist(&mut self, database: CollectionDatabase) {
        if let Ok(root) = &mut self.root {
            root.persist_all(&mut persistent::PersistentStore::new(database));
        }
    }

    /// ID of the selected profile. `None` iff the list is empty
    pub fn selected_profile_id(&self) -> Option<&ProfileId> {
        if let Ok(root) = &self.root {
            root.selected_profile_id()
        } else {
            None
        }
    }

    /// Get a definition of the request that should be sent from the current
    /// recipe settings
    pub fn request_config(&self) -> Option<RequestConfig> {
        if let Ok(root) = &self.root {
            root.request_config()
        } else {
            None
        }
    }

    /// Get a map of overridden profile fields
    pub fn profile_overrides(&self) -> IndexMap<String, Template> {
        if let Ok(root) = &self.root {
            root.profile_overrides()
        } else {
            IndexMap::default()
        }
    }

    /// Update the displayed request based on a change in HTTP request state.
    /// If the update is not relevant to what's on screen (e.g. an unselected
    /// request was modified), this will do nothing.
    pub fn refresh_request(
        &mut self,
        store: &mut RequestStore,
        disposition: RequestDisposition,
    ) {
        if let Ok(root) = &mut self.root {
            root.refresh_request(store, disposition);
        }
    }

    /// Ask the user a [Question]
    pub fn question(&mut self, question: Question) {
        if let Ok(root) = &mut self.root {
            root.question(question);
        }
    }

    /// Display an error to the user in a modal
    pub fn error(&mut self, error: anyhow::Error) {
        if let Ok(root) = &mut self.root {
            root.error(error);
        }
    }

    /// Display an informational notification to the user
    pub fn notify(&mut self, message: impl ToString) {
        if let Ok(root) = &mut self.root {
            root.notify(message.to_string());
        }
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
        let Ok(root) = &mut self.root else {
            // No way to handle events in error mode
            return false;
        };

        // If we haven't done first render yet, don't drain the queue. This can
        // happen after a collection reload, because of the structure of the
        // main loop
        if !context.component_map.is_visible(root) {
            return false;
        }

        let mut handled = false;
        // It's possible for components to queue additional events, so keep
        // going until the queue is empty
        while let Some(event) = ViewContext::pop_event() {
            handled = true;
            trace_span!("Handling event", ?event).in_scope(|| {
                match root.update_all(&mut context, event) {
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

/// Container for the state the view needs to show a collection load error
#[derive(Debug)]
pub struct InvalidCollection {
    pub file: CollectionFile,
    pub error: Arc<CollectionError>,
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

/// Hint to the view about how an HTTP request state update should be handled
#[derive(Debug, PartialEq)]
pub enum RequestDisposition {
    /// A request was changed in some way. If the request is visible, the UI
    /// will need to be updated.
    Change(RequestId),
    /// Multiple requests were changed. If any of these requests is visible, the
    /// UI will need to be updated.
    ChangeAll(Vec<RequestId>),
    /// Updated request should be selected as the active request. The selection
    /// will only be made if the request matches the current recipe/profile. Use
    /// this when a new request is created or a new recipe/profile was selected.
    Select(RequestId),
    /// Select the prompt form pane. Use this when a new prompt is visible and
    /// needs a response from the user. If the prompting request is not
    /// currently selected, do *not* make any changes.
    OpenForm(RequestId),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_util::{TestTerminal, assert_events, terminal},
        view::test_util::{TestHarness, harness},
    };
    use rstest::rstest;
    use slumber_core::collection::Collection;
    use slumber_util::Factory;

    /// Test view handling and drawing during initial view setup
    #[rstest]
    fn test_initial_draw(harness: TestHarness, terminal: TestTerminal) {
        let collection = Collection::factory(());
        let mut view = View::new(
            Ok(collection.into()),
            harness.database.clone(),
            harness.messages_tx(),
        );

        // Initial events
        assert_events!(
            Event::Emitted { .. }, // Recipe list selection
            Event::Emitted { .. }, // Primary pane selection
        );

        // Events should *still* be in the queue, because we haven't drawn yet
        let mut component_map = ComponentMap::default();
        let mut persisent_store = harness.persistent_store();
        let mut request_store = harness.request_store_mut();
        view.handle_events(UpdateContext {
            component_map: &component_map,
            persistent_store: &mut persisent_store,
            request_store: &mut request_store,
        });
        assert_events!(Event::Emitted { .. }, Event::Emitted { .. },);

        // Nothing new
        terminal.draw(|frame| component_map = view.draw(frame.buffer_mut()));
        assert_events!(Event::Emitted { .. }, Event::Emitted { .. },);

        // *Now* the queue is drained
        view.handle_events(UpdateContext {
            component_map: &component_map,
            persistent_store: &mut persisent_store,
            request_store: &mut request_store,
        });
        assert_events!();
    }
}
