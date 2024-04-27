//! Utilities for handling input events from users, as well as external async
//! events (e.g. HTTP responses)

use crate::{
    collection::{ProfileId, RecipeId},
    tui::{
        input::Action,
        view::{
            common::modal::{Modal, ModalPriority},
            state::{Notification, RequestState},
            Component,
        },
    },
};
use crossterm::event::{MouseEvent, MouseEventKind};
use std::{any::Any, cell::RefCell, collections::VecDeque, fmt::Debug};
use tracing::trace;

/// A UI element that can handle user/async input. This trait facilitates an
/// on-demand tree structure, where each element can furnish its list of
/// children. Events will be propagated bottom-up (i.e. leff-to-root), and each
/// element has the opportunity to consume the event so it stops bubbling.
pub trait EventHandler: Debug {
    /// Update the state of *just* this component according to the message.
    /// Returned outcome indicates what to do afterwards. Use [EventQueue] to
    /// queue subsequent events.
    fn update(&mut self, event: Event) -> Update {
        Update::Propagate(event)
    }

    /// Which, if any, of this component's children currently has focus? The
    /// focused component will receive first dibs on any update messages, in
    /// the order of the returned list. If none of the children consume the
    /// message, it will be passed to this component.
    fn children(&mut self) -> Vec<Component<&mut dyn EventHandler>> {
        Vec::new()
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
    thread_local! {
        /// This is used to access the event queue from anywhere in the view
        /// code. Since the view is all single-threaded, there should only ever
        /// be one instance of this thread local. All mutable accesses are
        /// restricted to the methods on the [EventQueue] type, so it's
        /// impossible for an outside caller to hold the ref cell open.
        ///
        /// The advantage of having this in a thread local is it makes it easy
        /// to queue additional events from anywhere, e.g. in event callbacks
        /// or during init.
        static INSTANCE: RefCell<EventQueue> = RefCell::default();
    }

    /// Queue a view event to be handled by the component tree
    pub fn push(event: Event) {
        trace!(?event, "Queueing view event");
        Self::INSTANCE
            .with_borrow_mut(move |context| context.0.push_back(event));
    }

    /// Pop an event off the queue
    pub fn pop() -> Option<Event> {
        Self::INSTANCE.with_borrow_mut(|context| context.0.pop_front())
    }

    /// Open a modal
    pub fn open_modal(modal: impl Modal + 'static, priority: ModalPriority) {
        Self::push(Event::OpenModal {
            modal: Box::new(modal),
            priority,
        });
    }

    /// Open a modal that implements `Default`, with low priority
    pub fn open_modal_default<T: Modal + Default + 'static>() {
        Self::open_modal(T::default(), ModalPriority::Low);
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
    /// Load a request from the database. Used to communicate from the recipe
    /// list to the parent, where more context is available.
    HttpLoadRequest,
    /// User wants to send a new request. Used to communicate from the recipe
    /// list to the parent, where more context is available.
    HttpSendRequest,
    /// Update our state based on external HTTP events
    HttpSetState {
        profile_id: Option<ProfileId>,
        recipe_id: RecipeId,
        #[debug(skip)]
        state: RequestState,
    },

    /// Show a modal to the user
    OpenModal {
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
    pub fn new<T: Any>(value: T) -> Event {
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
}

impl Event {
    /// Should this event immediately be killed, meaning it will never be
    /// handled by a component. This is used to filter out junk events that will
    /// never be handled, mostly to make debug logging cleaner.
    pub fn should_kill(&self) -> bool {
        use crossterm::event::Event;
        matches!(
            self,
            Self::Input {
                event: Event::FocusGained
                    | Event::FocusLost
                    | Event::Resize(_, _)
                    | Event::Mouse(MouseEvent {
                        kind: MouseEventKind::Moved,
                        ..
                    }),
                ..
            }
        )
    }

    /// Is this event pertinent to the component? Most events should be handled,
    /// but some (e.g. cursor events) need to be selectively filtered
    pub fn should_handle<T>(&self, component: &Component<T>) -> bool {
        use crossterm::event::Event;
        if let Self::Input { event, .. } = self {
            match event {
                Event::Key(_) | Event::Paste(_) => true,

                Event::Mouse(mouse_event) => {
                    // Check if the mouse is over the component
                    component.intersects(mouse_event)
                }

                // We expect everything else to have already been killed, but if
                // it made it through, handle it to be safe
                _ => true,
            }
        } else {
            true
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
    /// [EventQueue::push] for that. That will ensure the entire tree
    /// has a chance to respond to the entire event.
    Propagate(Event),
}
