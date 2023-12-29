//! Traits for rendering stuff

use ratatui::{layout::Rect, text::Span, Frame};
use std::fmt::Display;

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
    fn draw(&self, frame: &mut Frame, props: Props, area: Rect);
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

/// Marker trait th pull in a blanket impl of [Generate], which simply calls
/// [ToString::to_string] on the value to create a [ratatui::text::Span].
pub trait ToStringGenerate: Display {}

impl<T> Generate for &T
where
    T: ToStringGenerate,
{
    type Output<'this> = Span<'this>
    where
        Self: 'this;

    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this,
    {
        self.to_string().into()
    }
}
