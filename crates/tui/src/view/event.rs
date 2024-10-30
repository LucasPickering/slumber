//! Utilities for handling input events from users, as well as external async
//! events (e.g. HTTP responses)

use crate::view::{
    common::modal::Modal, context::UpdateContext, state::Notification,
    Component,
};
use persisted::{PersistedContainer, PersistedLazyRefMut, PersistedStore};
use slumber_config::Action;
use slumber_core::http::RequestId;
use std::{
    any::Any,
    collections::VecDeque,
    fmt::Debug,
    ops::{Deref, DerefMut},
};
use tracing::trace;

/// A UI element that can handle user/async input. This trait facilitates an
/// on-demand tree structure, where each element can furnish its list of
/// children. Events will be propagated bottom-up (i.e. leff-to-root), and each
/// element has the opportunity to consume the event so it stops bubbling.
pub trait EventHandler {
    /// Update the state of *just* this component according to the event.
    /// Returned outcome indicates whether the event was consumed, or it should
    /// be propagated to our parent. Use [EventQueue] to queue subsequent events,
    /// and the given message sender to queue async messages.
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

    /// A dynamically dispatched variant, which can hold any type. The name
    /// `Local` indicates that this event type is local to a specific
    /// branch of the component tree. This is useful for passing
    /// component-specific action types, e.g. when bubbling up a callback. Use
    /// [Self::local] or `LocalEvent::downcast_ref` to convert into the
    /// expected type.
    Local(Box<dyn LocalEvent>),
}

impl Event {
    /// Create a localized event of a dynamic type. See [Event::Local]
    pub fn new_local<T: LocalEvent>(value: T) -> Event {
        Event::Local(Box::new(value))
    }

    /// Get the mapped input action for this event, if any. A lot of components
    /// only handle mapped input events, so this is shorthand to check if this
    /// is one of those events.
    pub fn action(&self) -> Option<Action> {
        match self {
            Self::Input { action, .. } => *action,
            _ => None,
        }
    }

    /// If this is a local event (see [Event::Local]) of a specific type, return
    /// it. Otherwise return `None`.
    pub fn local<T: Any>(&self) -> Option<&T> {
        match self {
            Self::Local(local) => local.downcast_ref(),
            _ => None,
        }
    }
}

/// A wrapper trait for [Any] that also gives us access to the type's [Debug]
/// impl. This makes testing and logging much more effective, because we get the
/// value's underlying debug representation, rather than just `Event::Local(Any
/// {..})`.
pub trait LocalEvent: Any + Debug {
    // Workaround for trait upcasting
    // unstable: Delete this once we get trait upcasting
    // https://github.com/rust-lang/rust/issues/65991
    fn any(&self) -> &dyn Any;
}

impl<T: Any + Debug> LocalEvent for T {
    fn any(&self) -> &dyn Any {
        self
    }
}

impl dyn LocalEvent {
    /// Alias for `Any::downcast_ref`, to downcast into a concrete type
    pub fn downcast_ref<'a, T: Any>(self: &'a dyn LocalEvent) -> Option<&'a T> {
        self.any().downcast_ref()
    }
}

/// The result of a component state update operation. This corresponds to a
/// single input [Event].
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
