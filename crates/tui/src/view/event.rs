//! Utilities for handling input events from users, as well as external async
//! events (e.g. HTTP responses)

use crate::{
    input::InputEvent,
    util::Flag,
    view::{ViewContext, common::actions::MenuAction, util::format_type_name},
};
use ratatui::layout::Position;
use slumber_config::Action;
use slumber_core::{
    collection::{ProfileId, RecipeId},
    http::RequestId,
};
use std::{
    any::{self, Any},
    collections::VecDeque,
    fmt::Debug,
    marker::PhantomData,
    ops::Deref,
};
use terminput::ScrollDirection;
use tracing::trace;
use uuid::Uuid;

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
/// [Component::update](crate::view::component::Component::update), and
/// each component is responsible for modifying its own state accordingly.
/// Events can also trigger other events to propagate state changes, as well as
/// side-effect messages to trigger app-wide changes (e.g. launch a request).
///
/// This is conceptually different from [crate::Message] in that events are
/// restricted to the queue and handled in the main thread. Messages can be
/// queued asynchronously and are used to interact *between* threads.
#[derive(Debug)]
pub enum Event {
    /// See [BroadcastEvent]
    Broadcast(BroadcastEvent),

    /// User has requested to delete all requests for the current selected
    /// recipe. This will trigger a confirmation modal before the deletion
    DeleteRequests(DeleteTarget),

    /// A localized event emitted by a particular [Emitter] implementation.
    /// The event type here does not need to be unique because the emitter ID
    /// makes sure this will only be consumed by the intended recipient. Use
    /// [Emitter::emitted] to match on and consume this event type.
    Emitted {
        /// Who emitted this event?
        emitter_id: EmitterId,
        /// Store the type name for better debug messages
        emitter_type: String,
        event: Box<dyn LocalEvent>,
    },

    /// Input from the user, which may or may not be bound to an action. Most
    /// components just care about the action, but some require raw input
    Input(InputEvent),
}

impl Event {
    /// Convert to [EventMatch] so its methods can be used to match the event
    pub fn m(self) -> EventMatch {
        Some(self).into()
    }
}

impl From<BroadcastEvent> for Event {
    fn from(value: BroadcastEvent) -> Self {
        Self::Broadcast(value)
    }
}

/// A special type of event that is always propagated to all components
///
/// Broadcast events are for notifications rather than actions. They notify
/// components that an event has happened, allowing each component to update
/// its own state internally. They cannot be consumed. Check for a broadcast
/// event with [EventMatch::broadcast]
#[derive(Clone, Debug, PartialEq)]
pub enum BroadcastEvent {
    /// Rerender **all** template previews. This notifies all existing previews
    /// of a potential change that could affect their content. This is sent
    /// out when a profile field is modified because any template could contain
    /// that profile field.
    RefreshPreviews,

    /// Selected profile ID has changed. ID is `None` if there is no profile
    /// selected.
    SelectedProfile(Option<ProfileId>),

    /// Selected recipe ID has changed. ID is `None` if there is no recipe
    /// selected. This will be sent for selected folders as well.
    SelectedRecipe(Option<RecipeId>),

    /// Selected request ID has changed. ID is `None` if there is no request
    /// selected.
    SelectedRequest(Option<RequestId>),
}

/// Definition of what request(s) to start deletion for
#[derive(Debug)]
pub enum DeleteTarget {
    /// Delete the selected request
    Request,
    /// Delete all requests for the selected recipe
    Recipe {
        /// If `true`, delete all requests for all profiles. If `false`, delete
        /// just requests for the selected profile
        all_profiles: bool,
    },
}

/// Wrapper for matching an event to various expected cases.
///
/// Use the `From` impls to convert from `Event` and to `Option<Event>`.
pub struct EventMatch {
    event: Option<Event>,
}

impl EventMatch {
    /// Match and handle any event
    pub fn any(self, f: impl FnOnce(Event) -> Option<Event>) -> Self {
        let Some(event) = self.event else {
            return self;
        };
        f(event).into()
    }

    /// Match a [BroadcastEvent]. The event will always be propagated, even if
    /// the match succeeds.
    pub fn broadcast(self, f: impl FnOnce(BroadcastEvent)) -> Self {
        // Call the watcher. We ALWAYS propagate, because that's the purpose of
        // a broadcast event
        if let Some(Event::Broadcast(broadcast)) = &self.event {
            f(broadcast.clone()); // Clone needed so we can always propagate
        }
        self
    }

    /// Handle a left click event. Given position is the absolute position of
    /// the cursor. By default, click events are **always propagated**, even if
    /// handled by a child. This is to make it easy for parent and child to both
    /// grab focus when clicked (e.g. text box within a parent pane). If the
    /// action should *not* be propagated, call `propagate.unset()`.
    pub fn click(self, f: impl FnOnce(Position, &mut Flag)) -> Self {
        let Some(event) = self.event else {
            return self;
        };
        // Component logic is responsible for making sure a component only
        // receives a mouse event that's over the component
        if let Event::Input(InputEvent::Click { position }) = event {
            let mut propagate = Flag::default();
            propagate.set(); // Set by default!!
            f(position, &mut propagate);
            if *propagate { Some(event) } else { None }.into()
        } else {
            Some(event).into()
        }
    }

    /// Handle any scroll input event
    pub fn scroll(self, f: impl FnOnce(ScrollDirection)) -> Self {
        let Some(event) = self.event else {
            return self;
        };
        // Component logic is responsible for making sure a component only
        // receives a mouse event that's over the component
        if let Event::Input(InputEvent::Scroll { direction, .. }) = event {
            f(direction);
            None.into()
        } else {
            Some(event).into()
        }
    }

    /// Handle any key input event bound to an action. If the action is
    /// unhandled and the event should continue to be propagated, set the
    /// given flag.
    pub fn action(self, f: impl FnOnce(Action, &mut Flag)) -> Self {
        let Some(event) = self.event else {
            return self;
        };
        if let Event::Input(InputEvent::Key {
            action: Some(action),
            ..
        }) = &event
        {
            let mut propagate = Flag::default();
            f(*action, &mut propagate);
            if *propagate { Some(event) } else { None }.into()
        } else {
            Some(event).into()
        }
    }

    /// Handle an emitted event for a particular emitter. Each emitter should
    /// only be handled by a single parent, so this doesn't provide any way to
    /// propagate the event if it matches the emitter.
    ///
    /// Typically you'll need to pass a handle for the emitter here, in order
    /// to detach the emitter's lifetime from `self`, so that `self` can be used
    /// in the lambda.
    pub fn emitted<E>(self, emitter: Emitter<E>, f: impl FnOnce(E)) -> Self
    where
        E: LocalEvent,
    {
        let Some(event) = self.event else {
            return self;
        };
        match emitter.emitted(event) {
            Ok(output) => {
                f(output);
                None.into()
            }
            Err(event) => Some(event).into(),
        }
    }

    /// [Self::emitted], but the emitter is optional. If `None`, event is not
    /// consumed
    pub fn emitted_opt<E>(
        self,
        emitter: Option<Emitter<E>>,
        f: impl FnOnce(E),
    ) -> Self
    where
        E: LocalEvent,
    {
        if let Some(emitter) = emitter {
            self.emitted(emitter, f)
        } else {
            self
        }
    }
}

impl From<EventMatch> for Option<Event> {
    fn from(value: EventMatch) -> Self {
        value.event
    }
}

impl From<Option<Event>> for EventMatch {
    fn from(event: Option<Event>) -> Self {
        Self { event }
    }
}

/// A wrapper trait for [Any] that also gives us access to the type's [Debug]
/// impl. This makes testing and logging much more effective, because we get the
/// value's underlying debug representation, rather than just `Any {..}`.
pub trait LocalEvent: Any + Debug {}

impl<T: Any + Debug> LocalEvent for T {}

/// An emitter generates events of a particular type. This is used for
/// components that need to respond to actions performed on their children, e.g.
/// listen to select and submit events on a child list. It can also be used for
/// components to communicate with themselves from async actions, e.g. reporting
/// back the result of a modal interaction.
///
/// This is `!Send` it relies on the event queue in the ViewContext, which is
/// only present on the main thread.
#[derive(Debug, derive_more::Display)]
#[display("{id}")]
pub struct Emitter<T: ?Sized> {
    id: EmitterId,
    /// Store the emitted type so we can enforce it when it's emitted. *const
    /// makes this type !Send. Explicit unimplementation is unstable
    /// <https://github.com/rust-lang/rust/issues/68318>
    phantom: PhantomData<*const T>,
}

impl<T: ?Sized> Emitter<T> {
    fn new(id: EmitterId) -> Self {
        Self {
            id,
            phantom: PhantomData,
        }
    }
}

impl<T: Sized + LocalEvent> Emitter<T> {
    /// Push an event onto the event queue
    pub fn emit(&self, event: T) {
        ViewContext::push_event(Event::Emitted {
            emitter_id: self.id,
            emitter_type: format_type_name(any::type_name::<T>()),
            event: Box::new(event),
        });
    }

    /// Check if an event is an emitted event from this emitter, and return
    /// the emitted data if so
    pub fn emitted(&self, event: Event) -> Result<T, Event> {
        match event {
            Event::Emitted {
                emitter_id,
                event,
                emitter_type,
            } if emitter_id == self.id => {
                // This cast should be infallible because emitter IDs are unique
                // and each emitter can only emit one type
                Ok(*(event as Box<dyn Any>).downcast::<T>().unwrap_or_else(
                    |_| {
                        panic!(
                            "Incorrect emitted event type for emitter \
                        `{emitter_id}`. Expected type {}, received type \
                        {emitter_type}",
                            any::type_name::<T>()
                        )
                    },
                ))
            }
            _ => Err(event),
        }
    }

    /// Cast this to an emitter of `dyn LocalEvent`, so that it can emit events
    /// of any type. This should be used when emitting events of multiple types
    /// from the same spot. The original type must be known at consumption time,
    /// so [Self::emitted] can be used to downcast back.
    pub fn upcast(self) -> Emitter<dyn LocalEvent> {
        Emitter {
            id: self.id,
            phantom: PhantomData,
        }
    }

    /// Generate a menu action bound to this emitter. When fired, the action
    /// will emit an event through this emitter.
    pub fn menu(self, action: T, name: impl Into<String>) -> MenuAction {
        MenuAction::new(self, action, name)
    }
}

impl Emitter<dyn LocalEvent> {
    /// Push a type-erased event onto the event queue
    pub fn emit(&self, event: Box<dyn LocalEvent>) {
        ViewContext::push_event(Event::Emitted {
            emitter_id: self.id,
            // We lose the original type name :(
            emitter_type: format_type_name(any::type_name_of_val(&event)),
            // The event is already boxed, so do *not* double box it
            event,
        });
    }
}

// Manual impls needed to bypass bounds
impl<T> Copy for Emitter<T> {}

impl<T> Clone for Emitter<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Default for Emitter<T> {
    fn default() -> Self {
        Self::new(EmitterId::new())
    }
}

/// An emitter generates events of a particular type. This is used for
/// components that need to respond to actions performed on their children, e.g.
/// listen to select and submit events on a child list.
///
/// In most cases a component will emit only one type of event and therefore
/// one impl of this trait, but it's possible for a single component to have
/// multiple implementations. In the case of multiple implementations, the
/// component must store a different emitter for each implementation, since each
/// emitter is bound to a particular event type.
pub trait ToEmitter<E: LocalEvent> {
    fn to_emitter(&self) -> Emitter<E>;
}

// Implement ToEmitter through Deref
impl<T, E> ToEmitter<E> for T
where
    T: Deref,
    T::Target: ToEmitter<E>,
    E: LocalEvent,
{
    fn to_emitter(&self) -> Emitter<E> {
        self.deref().to_emitter()
    }
}

/// A unique ID to refer to a component instance that emits specialized events.
/// This is used by the consumer to confirm that the event came from a specific
/// instance, so that events from multiple instances of the same component type
/// cannot be mixed up. This should never be compared directly; use
/// [Emitter::emitted] instead.
#[derive(Copy, Clone, Debug, derive_more::Display, Eq, Hash, PartialEq)]
pub struct EmitterId(Uuid);

impl EmitterId {
    fn new() -> Self {
        Self(Uuid::new_v4())
    }
}
