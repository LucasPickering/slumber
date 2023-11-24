//! Traits for rendering stuff

use crate::tui::{
    input::InputEngine, message::MessageSender, view::ViewConfig,
};
use ratatui::{layout::Rect, Frame};

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
    fn draw(&self, context: &mut DrawContext, props: Props, area: Rect);
}

/// Global data that various components need during rendering. A mutable
/// reference to this is passed around to give access to the frame, but please
/// don't modify anything :)
#[derive(Debug)]
pub struct DrawContext<'a, 'f> {
    pub input_engine: &'a InputEngine,
    pub config: &'a ViewConfig,
    /// Allows draw functions to trigger async operations, if the drawn content
    /// needs some async calculation (e.g. template previews)
    pub messages_tx: MessageSender,
    pub frame: &'a mut Frame<'f>,
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
