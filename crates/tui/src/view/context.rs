use crate::{
    http::RequestStore,
    message::{Message, MessageSender},
    view::{
        component::RecipeOverrideStore,
        event::{Event, EventQueue},
        state::Notification,
        IntoModal,
    },
};
use slumber_core::{collection::Collection, db::CollectionDatabase};
use std::{cell::RefCell, sync::Arc};
use tracing::debug;

/// Thread-local context container, which stores mutable state needed in the
/// view thread. Until [TuiContext](crate::TuiContext), which stores
/// read-only state, this state can be mutable because it's not shared between
/// threads. Some pieces of this state *are* shared between threads, but that's
/// because they are internally thread-safe.
///
/// The main purpose of this is to prevent an absurd amount of plumbing required
/// to get all these individual pieces to every place they're needed in the
/// view code. We're leaning heavily on the fact that the view is
/// single-threaded here.
pub struct ViewContext {
    /// The request collection. This is immutable through the lifespan of the
    /// view; the entire view is rebuilt when the collection reloads.
    collection: Arc<Collection>,
    /// Persistence database. The TUI only ever needs to run DB ops related to
    /// our collection, so we can use a collection-restricted DB handle
    database: CollectionDatabase,
    /// An alternative persistence store just for recipe overrides. These
    /// values are only persisted within a single session, so they cannot
    /// use the DB
    recipe_override_store: RecipeOverrideStore,
    /// Queue of unhandled view events, which will be used to update view state
    event_queue: EventQueue,
    /// Sender to the async message queue, which is used to transmit data and
    /// trigger callbacks that require additional threading/background work.
    messages_tx: MessageSender,
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

    /// Initialize the view context for this thread
    pub fn init(
        collection: Arc<Collection>,
        database: CollectionDatabase,
        messages_tx: MessageSender,
    ) {
        debug!("Initializing view context");
        Self::INSTANCE.with_borrow_mut(|context| {
            *context = Some(Self {
                collection,
                database,
                recipe_override_store: Default::default(),
                event_queue: EventQueue::default(),
                messages_tx,
            })
        })
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

    /// Get a pointer to the request collection
    pub fn collection() -> Arc<Collection> {
        Self::with(|context| Arc::clone(&context.collection))
    }

    /// Execute a function with access to the database
    pub fn with_database<T>(f: impl FnOnce(&CollectionDatabase) -> T) -> T {
        Self::with(|context| f(&context.database))
    }

    /// Execute a function with immutable access to the [RecipeOverideStore]
    pub fn with_override_store<T>(
        f: impl FnOnce(&RecipeOverrideStore) -> T,
    ) -> T {
        Self::with(|context| f(&context.recipe_override_store))
    }

    /// Execute a function with mutable access to the [RecipeOverideStore]
    pub fn with_override_store_mut<T>(
        f: impl FnOnce(&mut RecipeOverrideStore) -> T,
    ) -> T {
        Self::with_mut(|context| f(&mut context.recipe_override_store))
    }

    /// Queue a view event to be handled by the component tree
    pub fn push_event(event: Event) {
        Self::with_mut(|context| context.event_queue.push(event));
    }

    /// Pop an event off the event queue
    pub fn pop_event() -> Option<Event> {
        Self::with_mut(|context| context.event_queue.pop())
    }

    /// Open a modal
    pub fn open_modal(modal: impl 'static + IntoModal) {
        Self::push_event(Event::OpenModal(Box::new(modal.into_modal())));
    }

    /// Queue an event to send an informational notification to the user
    pub fn notify(message: impl ToString) {
        let notification = Notification::new(message.to_string());
        Self::push_event(Event::Notify(notification));
    }

    /// Get a clone of the async message sender. Generally you should use
    /// [Self::send_message] instead, but in some contexts you need the whole
    /// sender.
    pub fn messages_tx() -> MessageSender {
        Self::with(|context| context.messages_tx.clone())
    }

    /// Send an async message on the channel
    pub fn send_message(message: Message) {
        Self::with(|context| context.messages_tx.send(message));
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
        })
    }
}

/// External data passed to [EventHandler::update]. This holds data that cannot
/// be held in [ViewContext], typically because of borrowing reasons.
pub struct UpdateContext<'a> {
    pub request_store: &'a mut RequestStore,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{assert_events, harness, TestHarness};
    use rstest::rstest;
    use slumber_core::assert_matches;

    #[rstest]
    fn test_event_queue(_harness: TestHarness) {
        assert_events!(); // Start empty

        ViewContext::push_event(Event::CloseModal { submitted: false });
        assert_events!(Event::CloseModal { submitted: false },);

        assert_matches!(
            ViewContext::pop_event(),
            Some(Event::CloseModal { .. })
        );
        assert_events!(); // Empty again
    }

    #[rstest]
    fn test_send_message(mut harness: TestHarness) {
        ViewContext::send_message(Message::CollectionStartReload);
        ViewContext::send_message(Message::CollectionEdit);
        assert_matches!(
            harness.pop_message_now(),
            Message::CollectionStartReload
        );
        assert_matches!(harness.pop_message_now(), Message::CollectionEdit);
    }
}
