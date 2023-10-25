//! The building blocks of the view

mod misc;
mod modal;
mod primary;
mod request;
mod response;
mod root;

pub use modal::{IntoModal, Modal};
pub use root::Root;

use crate::{
    config::RequestRecipeId,
    tui::{
        input::Action,
        message::Message,
        view::{
            component::root::RootMode,
            state::{Notification, RequestState},
            Frame, RenderContext,
        },
    },
};
use crossterm::event::Event;
use ratatui::prelude::Rect;
use std::fmt::{Debug, Display};

/// The main building block that makes up the view. This is modeled after React,
/// with some key differences:
///
/// - State can be exposed from child to parent
///   - This is arguably an anti-pattern, but it's a simple solution. Rust makes
///     it possible to expose only immutable references, so I think it's fine.
/// - State changes are managed via message passing rather that callbacks. See
///   [Component::update_all] and [Component::update]. This happens during the
///   message phase of the TUI.
/// - Rendering is provided by a separate trait: [Draw]
///
/// Requires `Display` impl for tracing. Typically the impl can just be the
/// component name.
pub trait Component: Debug + Display {
    /// Update the state of *just* this component according to the message.
    /// Returned outcome indicates what to do afterwards.
    fn update(&mut self, message: ViewMessage) -> UpdateOutcome {
        // By default just forward to our parent
        UpdateOutcome::Propagate(message)
    }

    /// Which, if any, of this component's children currently has focus? The
    /// focused component will receive first dibs on any update messages, in
    /// the order of the returned list. If none of the children consume the
    /// message, it will be passed to this component.
    fn focused_children(&mut self) -> Vec<&mut dyn Component> {
        Vec::new()
    }
}

/// Something that can be drawn onto screen as one or more TUI widgets.
///
/// Conceptually this is bascially part of `Component`, but having it separate
/// allows the `Props` associated type. Otherwise, there's no way to make a
/// trait object from `Component` across components with different props.
///
/// Props are additional temporary values that a struct may need in order
/// to render. Useful for passing down state values that are managed by
/// the parent, to avoid duplicating that state in the child. `Props` probably
/// would make more sense as an associated type, because you generally wouldn't
/// implement `Draw` for a single type with more than one value of `Props`. But
/// attaching a lifetime to the associated type makes using this in a trait
/// object very difficult (maybe impossible?). This is an easy shortcut.
pub trait Draw<Props = ()> {
    fn draw(
        &self,
        context: &RenderContext,
        props: Props,
        frame: &mut Frame,
        chunk: Rect,
    );
}

/// A trigger for state change in the view. Messages are handled by
/// [Component::update_all], and each component is responsible for modifying
/// its own state accordingly. Messages can also trigger other view messages
/// to propagate state changes, as well as side-effect messages to trigger
/// app-wide changes (e.g. launch a request).
///
/// This is conceptually different from [Message] in that view messages never
/// queued, they are handled immediately. Maybe "message" is a misnomer here and
/// we should rename this?
#[derive(Debug)]
pub enum ViewMessage {
    /// Input from the user, which may or may not correspond to a bound action.
    /// Most components just care about the action, but some require raw input
    Input {
        event: Event,
        action: Option<Action>,
    },

    // HTTP
    /// User wants to send a new request (upstream)
    HttpSendRequest,
    /// Update our state based on external HTTP events
    HttpSetState {
        recipe_id: RequestRecipeId,
        state: RequestState,
    },

    /// Change the root view mode
    OpenView(RootMode),

    /// Show a modal to the user
    OpenModal(Box<dyn Modal>),
    /// Close the current modal. This is useful for the contents of the modal
    /// to implement custom close triggers.
    CloseModal,

    /// Tell the user something informational
    Notify(Notification),
}

/// The result of a component state update operation. This corresponds to a
/// single input [ViewMessage].
#[derive(Debug)]
pub enum UpdateOutcome {
    /// The consuming component updated its state accordingly, and no further
    /// changes are necessary
    Consumed,
    /// The returned message should be passed to the parent component. This can
    /// mean one of two things:
    ///
    /// - The updated component did not handle the message, and it should
    ///   bubble up the tree
    /// - The updated component *did* make changes according to the message,
    ///   and is sending a related message up the tree for ripple-effect
    ///   changes
    ///
    /// This dual meaning is maybe a little janky. There's an argument that
    /// rippled changes should be a separate variant that would cause the
    /// caller to reset back to the bottom of the component tree. There's
    /// no immediate need for that though so I'm keeping it simpler for
    /// now.
    Propagate(ViewMessage),
    /// The component consumed the message, and wants to trigger an app-wide
    /// action in response to it. The action should be queued on the controller
    /// so it can be handled asyncronously.
    SideEffect(Message),
}
