//! Utilities for handling input events from users, as well as external async
//! events (e.g. HTTP responses)

use crate::{
    util::Flag,
    view::{
        common::modal::Modal, context::UpdateContext, state::Notification,
        Component, ViewContext,
    },
};
use derive_more::derive::Display;
use persisted::{PersistedContainer, PersistedLazyRefMut, PersistedStore};
use slumber_config::Action;
use slumber_core::http::RequestId;
use std::{
    any::{self, Any},
    collections::VecDeque,
    fmt::Debug,
    marker::PhantomData,
    ops::{Deref, DerefMut},
};
use tracing::trace;
use uuid::Uuid;

/// A UI element that can handle user/async input. This trait facilitates an
/// on-demand tree structure, where each element can furnish its list of
/// children. Events will be propagated bottom-up (i.e. leff-to-root), and each
/// element has the opportunity to consume the event so it stops bubbling.
pub trait EventHandler {
    /// Update the state of *just* this component according to the event.
    /// Returned outcome indicates whether the event was consumed, or it should
    /// be propagated to our parent. Use [EventQueue] to queue subsequent
    /// events, and the given message sender to queue async messages.
    fn update(&mut self, _: &mut UpdateContext, event: Event) -> Update {
        Update::Propagate(event)
    }

    /// Get **all** children of this component. This includes children that are
    /// not currently visible, and ones that are out of focus, meaning they
    /// shouldn't receive keyboard events. The event handling infrastructure is
    /// responsible for filtering out children that shouldn't receive events.
    ///
    /// The event handling sequence goes something like:
    /// - Get list of children
    /// - Filter out children that aren't visible
    /// - For keyboard events, filter out children that aren't in focus (mouse
    ///   events can still be handled by unfocused components)
    /// - Pass the event to the first child in the list
    ///     - If it consumes the event, stop
    ///     - If it propagates, move on to the next child, and so on
    /// - If none of the children consume the event, go up the tree to the
    ///   parent and try again.
    fn children(&mut self) -> Vec<Component<Child<'_>>> {
        Vec::new()
    }
}

/// Enable `Component<Option<T>>` with an empty event handler
impl<T: EventHandler> EventHandler for Option<T> {
    fn update(&mut self, context: &mut UpdateContext, event: Event) -> Update {
        if let Some(inner) = self.as_mut() {
            inner.update(context, event)
        } else {
            Update::Propagate(event)
        }
    }

    fn children(&mut self) -> Vec<Component<Child<'_>>> {
        if let Some(inner) = self.as_mut() {
            inner.children()
        } else {
            vec![]
        }
    }
}

// We can't do a blanket impl of EventHandler based on DerefMut because of the
// PersistedLazy's custom ToChild impl, which interferes with the blanket
// ToChild impl

impl<'a> EventHandler for Child<'a> {
    fn update(&mut self, context: &mut UpdateContext, event: Event) -> Update {
        self.deref_mut().update(context, event)
    }

    fn children(&mut self) -> Vec<Component<Child<'_>>> {
        self.deref_mut().children()
    }
}

impl<'a, S, K, C> EventHandler for PersistedLazyRefMut<'a, S, K, C>
where
    S: PersistedStore<K>,
    K: persisted::PersistedKey,
    K::Value: Debug + PartialEq,
    C: EventHandler + PersistedContainer<Value = K::Value>,
{
    fn update(&mut self, context: &mut UpdateContext, event: Event) -> Update {
        self.deref_mut().update(context, event)
    }

    fn children(&mut self) -> Vec<Component<Child<'_>>> {
        self.deref_mut().children()
    }
}

/// A wrapper for a dynamically dispatched [EventHandler]. This is used to
/// return a collection of event handlers from [EventHandler::children]. Almost
/// all cases will use the [Borrowed](Self::Borrowed) variant, but
/// [Owned](Self::Owned) is useful for types that need to wrap the mutable
/// reference in some type of guard. See [ToChild].
pub enum Child<'a> {
    Borrowed(&'a mut dyn EventHandler),
    Owned(Box<dyn 'a + EventHandler>),
}

impl<'a> Deref for Child<'a> {
    type Target = dyn 'a + EventHandler;

    fn deref(&self) -> &Self::Target {
        match self {
            Child::Borrowed(inner) => *inner,
            Child::Owned(inner) => inner.deref(),
        }
    }
}

impl<'a> DerefMut for Child<'a> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self {
            Child::Borrowed(inner) => *inner,
            Child::Owned(inner) => inner.deref_mut(),
        }
    }
}

/// Abstraction to convert a component type into [Child], which is a wrapper for
/// a trait object. For 99% of components the blanket implementation will cover
/// this. This only needs to be implemented manually for types that need an
/// extra step to extract mutable data.
pub trait ToChild {
    fn to_child_mut(&mut self) -> Child<'_>;
}

impl<T: EventHandler> ToChild for T {
    fn to_child_mut(&mut self) -> Child<'_> {
        Child::Borrowed(self)
    }
}

/// A mutable reference to the contents of [PersistedLazy] must be wrapped in
/// [PersistedLazyRefMut], which requires us to return an owned child rather
/// than a borrowed one.
impl<S, K, C> ToChild for persisted::PersistedLazy<S, K, C>
where
    S: PersistedStore<K>,
    K: persisted::PersistedKey,
    K::Value: Debug + PartialEq,
    C: EventHandler + PersistedContainer<Value = K::Value>,
{
    fn to_child_mut(&mut self) -> Child<'_> {
        Child::Owned(Box::new(self.get_mut()))
    }
}

/// A queue of view events. Any component within the view can add to this, and
/// outside the view (e.g. from the main loop) it can be added to via the view.
///
/// This is drained by the view, which is responsible for passing those events
/// down the component tree.
#[derive(Default)]
pub struct EventQueue(VecDeque<Event>);

impl EventQueue {
    /// Queue a view event to be handled by the component tree
    pub fn push(&mut self, event: Event) {
        trace!(?event, "Queueing view event");
        self.0.push_back(event);
    }

    /// Pop an event off the queue
    pub fn pop(&mut self) -> Option<Event> {
        self.0.pop_front()
    }

    /// Collect references to each event into a vector, for asserting on it
    #[cfg(test)]
    pub fn to_vec(&self) -> Vec<&Event> {
        self.0.iter().collect()
    }
}

/// A trigger for state change in the view. Events are handled by
/// [EventHandler::update], and each component is responsible for modifying its
/// own state accordingly. Events can also trigger other events to propagate
/// state changes, as well as side-effect messages to trigger app-wide changes
/// (e.g. launch a request).
///
/// This is conceptually different from [crate::Message] in that events are
/// restricted to the queue and handled in the main thread. Messages can be
/// queued asynchronously and are used to interact *between* threads.
#[derive(derive_more::Debug)]
pub enum Event {
    /// Input from the user, which may or may not correspond to a bound action.
    /// Most components just care about the action, but some require raw input
    Input {
        event: crossterm::event::Event,
        action: Option<Action>,
    },

    // HTTP
    /// Load a request from the database. If the ID is given, load that
    /// specific request. If not, get the most recent for the current
    /// profile+recipe.
    HttpSelectRequest(Option<RequestId>),

    /// Show a modal to the user
    OpenModal(Box<dyn Modal>),
    /// Close the current modal. This is useful for the contents of the modal
    /// to implement custom close triggers
    CloseModal {
        /// Some modals have a concept of submission, and want to execute
        /// certain one-time code during close, conditional on whether the
        /// modal was submitted or cancelled. For modals without submissions,
        /// this is `false`.
        submitted: bool,
    },

    /// Tell the user something informational
    Notify(Notification),

    /// A localized event emitted by a particular [Emitter] implementation.
    /// The event type here does not need to be unique because the emitter ID
    /// makes sure this will only be consumed by the intended recipient. Use
    /// [Emitter::emitted] to match on and consume this event type.
    Emitted {
        emitter: EmitterId,
        /// Store the type name for better debug messages
        emitter_type: &'static str,
        event: Box<dyn LocalEvent>,
    },
}

impl Event {
    /// Get a matcher that can be used to chain method calls together for
    /// ergonomically matching events
    pub fn m(self) -> Update {
        Update::Propagate(self)
    }
}

/// The result of a component state update operation. This corresponds to a
/// single input [Event]. This is the output of an event handler's `update`
/// call, but can also be used within an event handler to match against various
/// event types using the provided methods.
#[derive(Debug)]
pub enum Update {
    /// The consuming component updated its state accordingly, and no further
    /// changes are necessary
    Consumed,
    /// The message was not consumed by this component, and should be passed to
    /// the parent component. While technically possible, this should *not* be
    /// used to trigger additional events. Instead, use
    /// [ViewContext::push_event](crate::view::ViewContext::push_event)
    /// for that. That will ensure the entire tree has a chance to respond to
    /// the entire event.
    Propagate(Event),
}

impl Update {
    /// Match and handle any event
    pub fn any(self, f: impl FnOnce(Event) -> Update) -> Self {
        let Self::Propagate(event) = self else {
            return self;
        };
        f(event)
    }

    /// Handle any input event bound to an action. If the action is unhandled
    /// and the event should continue to be propagated, set the given flag.
    pub fn action(self, f: impl FnOnce(Action, &mut Flag)) -> Self {
        let Self::Propagate(event) = self else {
            return self;
        };
        if let Event::Input {
            action: Some(action),
            ..
        } = &event
        {
            let mut propagate = Flag::default();
            f(*action, &mut propagate);
            if *propagate {
                Self::Propagate(event)
            } else {
                Self::Consumed
            }
        } else {
            Self::Propagate(event)
        }
    }

    /// Handle an emitted event for a particular emitter. Each emitter should
    /// only be handled by a single parent, so this doesn't provide any way to
    /// propagate the event if it matches the emitter.
    ///
    /// Typically you'll need to pass a handle for the emitter here, in order
    /// to detach the emitter's lifetime from `self`, so that `self` can be used
    /// in the lambda.
    pub fn emitted<T: Emitter>(
        self,
        emitter: T,
        f: impl FnOnce(T::Emitted),
    ) -> Self {
        let Self::Propagate(event) = self else {
            return self;
        };
        match emitter.emitted(event) {
            Ok(output) => {
                f(output);
                Self::Consumed
            }
            Err(event) => Self::Propagate(event),
        }
    }
}

/// A wrapper trait for [Any] that also gives us access to the type's [Debug]
/// impl. This makes testing and logging much more effective, because we get the
/// value's underlying debug representation, rather than just `Any {..}`.
pub trait LocalEvent: Any + Debug {
    // Workaround for trait upcasting
    // unstable: Delete this once we get trait upcasting
    // https://github.com/rust-lang/rust/issues/65991
    fn into_any(self: Box<Self>) -> Box<dyn Any>;
}

impl<T: Any + Debug> LocalEvent for T {
    fn into_any(self: Box<Self>) -> Box<dyn Any> {
        self
    }
}

impl dyn LocalEvent {
    /// Alias for `Any::downcast`, to downcast into a concrete type
    pub fn downcast<T: Any>(self: Box<dyn LocalEvent>) -> Option<T> {
        self.into_any().downcast().map(|b| *b).ok()
    }
}

/// An emitter generates events of a particular type. This is used for
/// components that need to respond to actions performed on their children, e.g.
/// listen to select and submit events on a child list.
pub trait Emitter {
    /// The type of event emitted by this emitter. This will be converted into
    /// a trait object when pushed onto the event queue, and converted back by
    /// [Self::emitted]
    type Emitted: LocalEvent;

    /// Return a unique ID for this emitter. Unlike components, where the
    /// `Component` wrapper holds the ID, emitters are responsible for holding
    /// their own IDs. This makes the ergonomics much simpler, and allows for
    /// emitters to be used in contexts where a component wrapper doesn't exist.
    fn id(&self) -> EmitterId;

    /// Get the name of the implementing type, for logging/debugging
    fn type_name(&self) -> &'static str {
        any::type_name::<Self>()
    }

    /// Push an event onto the event queue
    fn emit(&self, event: Self::Emitted) {
        ViewContext::push_event(Event::Emitted {
            emitter: self.id(),
            emitter_type: self.type_name(),
            event: Box::new(event),
        });
    }

    /// Get a lightweight version of this emitter, with the same type and ID but
    /// datched from this lifetime. Useful for both emitting and consuming
    /// emissions from across task boundaries, modals, etc.
    fn handle(&self) -> EmitterHandle<Self::Emitted> {
        EmitterHandle {
            id: self.id(),
            phantom: PhantomData,
        }
    }

    /// Check if an event is an emitted event from this emitter, and return
    /// the emitted data if so
    fn emitted(&self, event: Event) -> Result<Self::Emitted, Event> {
        match event {
            Event::Emitted {
                emitter,
                event,
                emitter_type,
            } if emitter == self.id() => {
                // This cast should be infallible because emitter IDs are unique
                // and each emitter can only emit one type
                Ok(event.downcast().unwrap_or_else(|| {
                    panic!(
                        "Incorrect emitted event type for emitter `{emitter}`. \
                        Expected type {}, received type {emitter_type}",
                        any::type_name::<Self::Emitted>()
                    )
                }))
            }
            _ => Err(event),
        }
    }
}

impl<T> Emitter for T
where
    T: Deref,
    T::Target: Emitter,
{
    type Emitted = <<T as Deref>::Target as Emitter>::Emitted;

    fn id(&self) -> EmitterId {
        self.deref().id()
    }
}

/// A unique ID to refer to a component instance that emits specialized events.
/// This is used by the consumer to confirm that the event came from a specific
/// instance, so that events from multiple instances of the same component type
/// cannot be mixed up. This should never be compared directly; use
/// [Emitter::emitted] instead.
#[derive(Copy, Clone, Debug, Display, Eq, Hash, PartialEq)]
pub struct EmitterId(Uuid);

impl EmitterId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Empty emitted ID, to be used when no emitter is present
    pub fn nil() -> Self {
        Self(Uuid::nil())
    }
}

/// Generate a new random ID
impl Default for EmitterId {
    fn default() -> Self {
        Self::new()
    }
}

/// A lightweight copy of an emitter that can be passed between threads or
/// callbacks. This has the same emitting capability as the source emitter
/// because it contains its ID and is bound to its emitted event type. Generate
/// via [Emitter::handle].
///
/// It would be good to impl `!Send` for this type because it shouldn't be
/// passed between threads, but there is one use case where it needs to be Send
/// to be passed to the main loop via Message, but doesn't actually change
/// threads. !Send is more correct because if you try to emit from another
/// thread it'll panic, because the event queue isn't available there.
#[derive(Debug)]
pub struct EmitterHandle<T> {
    id: EmitterId,
    phantom: PhantomData<T>,
}

// Manual impls needed to bypass bounds
impl<T> Copy for EmitterHandle<T> {}

impl<T> Clone for EmitterHandle<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: LocalEvent> Emitter for EmitterHandle<T> {
    type Emitted = T;

    fn id(&self) -> EmitterId {
        self.id
    }
}
