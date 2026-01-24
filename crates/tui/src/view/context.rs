use crate::{
    http::RequestStore,
    input::InputBindings,
    message::{Message, MessageSender},
    view::{
        component::ComponentMap,
        event::{Event, EventQueue},
        persistent::PersistentStore,
        styles::Styles,
    },
};
use futures::FutureExt;
use slumber_config::{Action, Config};
use slumber_core::{collection::Collection, database::CollectionDatabase};
use std::{cell::RefCell, fmt::Display, sync::Arc};
use tracing::debug;

/// Thread-local context container, which stores mutable state needed in the
/// view thread
///
/// The main purpose of this is to prevent an absurd amount of plumbing required
/// to get all these individual pieces to every place they're needed in the view
/// code. We're leaning heavily on the fact that the view is single-threaded
/// here.
pub struct ViewContext {
    /// App-level configuration
    config: Arc<Config>,
    /// The request collection. This is immutable through the lifespan of the
    /// view; the entire view is rebuilt when the collection reloads.
    collection: Arc<Collection>,
    /// Persistence database. The TUI only ever needs to run DB ops related to
    /// our collection, so we can use a collection-restricted DB handle
    database: CollectionDatabase,
    /// Queue of unhandled view events, which will be used to update view state
    event_queue: EventQueue,
    /// Input:action bindings. Used in the view to show hotkey help/suggestions
    input_bindings: InputBindings,
    /// Sender to the async message queue, which is used to transmit data and
    /// trigger callbacks that require additional threading/background work.
    messages_tx: MessageSender,
    /// Visual styles, derived from the theme
    styles: Styles,
}

impl ViewContext {
    thread_local! {
        /// This is used to access the view context from anywhere in the view
        /// code. Since the view is all single-threaded, there should only ever
        /// be one instance of this thread local (aside from tests). All mutable
        /// accesses are restricted to the methods on this struct type, so it's
        /// impossible for an outside caller to hold the ref cell open. This is
        /// only `None` if the context hasn't yet been initialized for the
        /// thread.
        ///
        /// Technically we could use a global static instead of a thread local
        /// as far as the app is concerned, since we only initialize it on one
        /// thread anyway. But that makes testing pretty much impossible, since
        /// all tests would share the same value.
        static INSTANCE: RefCell<Option<ViewContext>> = RefCell::default();
    }

    /// Initialize or overwrite the view context
    pub fn init(
        config: Arc<Config>,
        collection: Arc<Collection>,
        database: CollectionDatabase,
        messages_tx: MessageSender,
    ) {
        debug!("Initializing view context");
        let styles = Styles::new(&config.tui.theme);
        let input_bindings =
            InputBindings::new(config.tui.input_bindings.clone());
        Self::INSTANCE.with_borrow_mut(|context| {
            *context = Some(Self {
                config,
                collection,
                database,
                event_queue: EventQueue::default(),
                input_bindings,
                messages_tx,
                styles,
            });
        });
    }

    /// Execute a function with read-only access to the context
    fn with<T>(f: impl FnOnce(&ViewContext) -> T) -> T {
        Self::INSTANCE.with_borrow(|context| {
            let context =
                context.as_ref().expect("View context not initialized");
            f(context)
        })
    }

    /// Execute a function with mutable access to the context
    fn with_mut<T>(f: impl FnOnce(&mut ViewContext) -> T) -> T {
        Self::INSTANCE.with_borrow_mut(|context| {
            let context =
                context.as_mut().expect("View context not initialized");
            f(context)
        })
    }

    /// Shortcut for [InputBindings::add_hint]
    pub fn add_binding_hint(label: impl Display, action: Action) -> String {
        Self::with(|context| context.input_bindings.add_hint(label, action))
    }

    /// Shortcut for [InputBindings::binding_display]
    pub fn binding_display(action: Action) -> String {
        Self::with(|context| context.input_bindings.binding_display(action))
    }

    /// Get the request collection
    pub fn collection() -> Arc<Collection> {
        Self::with(|context| Arc::clone(&context.collection))
    }

    /// Get the global configuration
    pub fn config() -> Arc<Config> {
        Self::with(|context| Arc::clone(&context.config))
    }

    /// Queue a view event to be handled by the component tree
    pub fn push_event(event: impl Into<Event>) {
        Self::with_mut(|context| context.event_queue.push(event.into()));
    }

    /// Pop an event off the event queue
    pub fn pop_event() -> Option<Event> {
        Self::with_mut(|context| context.event_queue.pop())
    }

    /// Get a clone of the async message sender. Generally you should use
    /// [Self::send_message] instead, but in some contexts you need the whole
    /// sender.
    pub fn messages_tx() -> MessageSender {
        Self::with(|context| context.messages_tx.clone())
    }

    /// Send an async message on the channel
    pub fn send_message(message: impl Into<Message>) {
        Self::with(|context| context.messages_tx.send(message));
    }

    /// Spawn a future in a new task on the main thread. See [Message::Spawn]
    pub fn spawn(future: impl 'static + Future<Output = ()>) {
        Self::send_message(Message::Spawn(future.boxed_local()));
    }

    /// Get a clone of the stylesheet
    pub fn styles() -> Styles {
        // Not sure how expensive this clone is. My guess is it's negligible,
        // but at some point I might profile it
        Self::with(|context| context.styles.clone())
    }

    /// Execute a function with access to the database
    pub fn with_database<T>(f: impl FnOnce(&CollectionDatabase) -> T) -> T {
        Self::with(|context| f(&context.database))
    }

    /// Execute a function with access to the input bindings
    pub fn with_input<T>(f: impl FnOnce(&InputBindings) -> T) -> T {
        Self::with(|context| f(&context.input_bindings))
    }
}

/// Test-only utils
#[cfg(test)]
impl ViewContext {
    /// Execute a function with read-only access to the event queue
    pub fn inspect_event_queue(f: impl FnOnce(&[&Event])) {
        Self::with(|context| {
            let refs: Vec<_> = context.event_queue.to_vec();
            f(refs.as_slice());
        });
    }
}

/// External data passed to
/// [ComponentExt::update](crate::view::component::ComponentExt). This holds
/// data that cannot be held in [ViewContext], typically because of borrowing
/// reasons.
pub struct UpdateContext<'a> {
    /// Visible components from the last draw phase
    pub component_map: &'a ComponentMap,
    /// Access to the persistent and session stores. Most interactions with
    /// this are done in
    /// [Component::persist](super::component::Component::persist), but
    /// sometimes components need to directly modify the store.
    pub persistent_store: &'a mut PersistentStore,
    /// Request state
    pub request_store: &'a mut RequestStore,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_util::assert_events,
        view::{
            event::DeleteTarget,
            test_util::{TestHarness, harness},
        },
    };
    use rstest::rstest;
    use slumber_util::assert_matches;

    #[rstest]
    fn test_event_queue(_harness: TestHarness) {
        assert_events!(); // Start empty

        ViewContext::push_event(Event::DeleteRequests(DeleteTarget::Request));
        assert_events!(Event::DeleteRequests(DeleteTarget::Request));

        assert_matches!(
            ViewContext::pop_event(),
            Some(Event::DeleteRequests(DeleteTarget::Request))
        );
        assert_events!(); // Empty again
    }

    #[rstest]
    fn test_send_message(mut harness: TestHarness) {
        ViewContext::send_message(Message::CollectionStartReload);
        ViewContext::send_message(Message::CollectionEdit { location: None });
        assert_matches!(
            harness.messages().pop_now(),
            Message::CollectionStartReload
        );
        assert_matches!(
            harness.messages().pop_now(),
            Message::CollectionEdit { .. }
        );
    }
}
