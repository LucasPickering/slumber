use crate::tui::{
    message::{Message, MessageSender},
    view::{
        common::modal::Modal,
        event::{Event, EventQueue},
        ModalPriority,
    },
};
use persisted::PersistedStore;
use serde::{de::DeserializeOwned, Serialize};
use slumber_core::db::CollectionDatabase;
use std::{cell::RefCell, fmt::Debug};

/// Thread-local context container, which stores mutable state needed in the
/// view thread. Until [TuiContext](crate::tui::TuiContext), which stores
/// read-only state, this state can be mutable because it's not shared between
/// threads. Some pieces of this state *are* shared between threads, but that's
/// because they are internally thread-safe.
///
/// The main purpose of this is to prevent an absurd amount of plumbing required
/// to get all these individual pieces to every place they're needed in the
/// view code. We're leaning heavily on the fact that the view is
/// single-threaded here.
pub struct ViewContext {
    /// Persistence database. The TUI only ever needs to run DB ops related to
    /// our collection, so we can use a collection-restricted DB handle
    database: CollectionDatabase,
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
    pub fn init(database: CollectionDatabase, messages_tx: MessageSender) {
        Self::INSTANCE.with_borrow_mut(|context| {
            *context = Some(Self {
                database,
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

    /// Execute a function with access to the database
    pub fn with_database<T>(f: impl FnOnce(&CollectionDatabase) -> T) -> T {
        Self::with(|context| f(&context.database))
    }

    /// Queue a view event to be handled by the component tree
    pub fn push_event(event: Event) {
        Self::with_mut(|context| context.event_queue.push(event))
    }

    /// Pop an event off the event queue
    pub fn pop_event() -> Option<Event> {
        Self::with_mut(|context| context.event_queue.pop())
    }

    /// Open a modal
    pub fn open_modal(modal: impl Modal + 'static, priority: ModalPriority) {
        Self::push_event(Event::OpenModal {
            modal: Box::new(modal),
            priority,
        });
    }

    /// Open a modal that implements `Default`, with low priority
    pub fn open_modal_default<T: Modal + Default + 'static>() {
        Self::open_modal(T::default(), ModalPriority::Low);
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

/// Wrapper for [persisted::PersistedKey] that applies additional bounds
/// necessary for our store
pub trait PersistedKey: Debug + Serialize + persisted::PersistedKey {}
impl<T: Debug + Serialize + persisted::PersistedKey> PersistedKey for T {}

/// Wrapper for [persisted::Persisted] bound to our store
pub type Persisted<K> = persisted::Persisted<ViewContext, K>;

/// Wrapper for [persisted::PersistedLazy] bound to our store
pub type PersistedLazy<K, C> = persisted::PersistedLazy<ViewContext, K, C>;

/// Persist UI state via the database. We have to be able to serialize keys to
/// insert and lookup. We have to serialize values to insert, and deserialize
/// them to retrieve.
impl<K> PersistedStore<K> for ViewContext
where
    K: PersistedKey,
    K::Value: Debug + Serialize + DeserializeOwned,
{
    fn load_persisted(key: &K) -> Option<K::Value> {
        Self::with_database(|database| database.get_ui((K::type_name(), key)))
            // Error is already traced in the DB, nothing to do with it here
            .ok()
            .flatten()
    }

    fn store_persisted(key: &K, value: K::Value) {
        Self::with_database(|database| {
            database.set_ui((K::type_name(), key), value)
        })
        // Error is already traced in the DB, nothing to do with it here
        .ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::test_util::{assert_events, harness, TestHarness};
    use rstest::rstest;
    use slumber_core::assert_matches;

    #[rstest]
    fn test_event_queue(_harness: TestHarness) {
        assert_events!(); // Start empty

        ViewContext::push_event(Event::new_local(3u32));
        ViewContext::push_event(Event::CloseModal);
        assert_events!(
            Event::Local(event) if event.downcast_ref::<u32>() == Some(&3),
            Event::CloseModal,
        );

        assert_matches!(ViewContext::pop_event(), Some(Event::Local(_)));
        assert_matches!(ViewContext::pop_event(), Some(Event::CloseModal));
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
