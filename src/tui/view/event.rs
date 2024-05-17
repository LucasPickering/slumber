//! Utilities for handling input events from users, as well as external async
//! events (e.g. HTTP responses)

use crate::{
    http::RequestId,
    tui::{
        input::Action,
        view::{
            common::modal::{Modal, ModalPriority},
            state::{Notification, RequestState},
            Component,
        },
    },
};
use std::{any::Any, collections::VecDeque, fmt::Debug};
use tracing::trace;

/// A UI element that can handle user/async input. This trait facilitates an
/// on-demand tree structure, where each element can furnish its list of
/// children. Events will be propagated bottom-up (i.e. leff-to-root), and each
/// element has the opportunity to consume the event so it stops bubbling.
pub trait EventHandler: Debug {
    /// Update the state of *just* this component according to the event.
    /// Returned outcome indicates whether the event was consumed, or it should
    /// be propgated to our parent. Use [EventQueue] to queue subsequent events,
    /// and the given message sender to queue async messages.
    fn update(&mut self, event: Event) -> Update {
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
    /// events can still be handled by unfocused components)
    /// - Pass the event to the first child in the list
    ///     - If it consumes the event, stop
    ///     - If it propagates, move on to the next child, and so on
    /// - If none of the children consume the event, go up the tree to the
    ///   parent and try again.
    fn children(&mut self) -> Vec<Component<&mut dyn EventHandler>> {
        Vec::new()
    }
}

impl EventHandler for &mut dyn EventHandler {
    fn update(&mut self, event: Event) -> Update {
        (*self).update(event)
    }

    fn children(&mut self) -> Vec<Component<&mut dyn EventHandler>> {
        (*self).children()
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
/// [Component::update], and each component is responsible for modifying
/// its own state accordingly. Events can also trigger other events
/// to propagate state changes, as well as side-effect messages to trigger
/// app-wide changes (e.g. launch a request).
///
/// This is conceptually different from [crate::tui::Message] in that events are
/// restricted to the queue and handled in the main thread. Messages can be
/// queued asyncronously and are used to interact *between* threads.
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
    /// Update the state of an in-progress HTTP request
    HttpSetState(RequestState),

    /// Show a modal to the user
    OpenModal {
        #[debug(skip)]
        modal: Box<dyn Modal>,
        priority: ModalPriority,
    },
    /// Close the current modal. This is useful for the contents of the modal
    /// to implement custom close triggers.
    CloseModal,

    /// Tell the user something informational
    Notify(Notification),

    /// A dynamically dispatched variant, which can hold any type. This is
    /// useful for passing component-specific action types, e.g. when bubbling
    /// up a callback. Use [std::any::Any::downcast_ref] to convert into the
    /// expected type.
    Other(Box<dyn Any>),
}

impl Event {
    /// Create a dynamic "other" variant
    pub fn new_other<T: Any>(value: T) -> Event {
        Event::Other(Box::new(value))
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

    /// Get a dynamic "other" variant, if this event is one
    pub fn other<T: Any>(&self) -> Option<&T> {
        match self {
            Self::Other(other) => other.downcast_ref(),
            _ => None,
        }
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
    /// [ViewContext::push_event] for that. That will ensure the entire tree
    /// has a chance to respond to the entire event.
    Propagate(Event),
}
