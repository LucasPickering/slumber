//! Traits for rendering stuff

use ratatui::{layout::Rect, text::Span, Frame};
use std::{fmt::Display, ops::Deref};

/// Something that can be drawn onto screen as one or more TUI widgets.
///
/// Conceptually this is bascially part of `Component`, but having it separate
/// allows the `Props` associated type. Otherwise, there's no way to make a
/// trait object from `Component` across components with different props.
///
/// Props are additional temporary values that a struct may need in order
/// to render. Useful for passing down state values that are managed by
/// the parent, to avoid duplicating that state in the child. In most
/// cases, `Props` would make more sense as an associated type, but there are
/// some component types (e.g. `SelectState`) that have multiple `Draw` impls.
/// Using an associated type also makes prop types with lifetimes much less
/// ergonomic.
pub trait Draw<Props = ()> {
    /// Draw the component into the frame. This generally should not be called
    /// directly. Instead, use
    /// [Component::draw](crate::tui::view::component::Component::draw), which
    /// will handle additional metadata management before defering to this
    /// method for the actual draw.
    fn draw(&self, frame: &mut Frame, props: Props, metadata: DrawMetadata);
}

/// Allow transparenting drawing through Deref impls
impl<T, Props> Draw<Props> for T
where
    T: Deref,
    T::Target: Draw<Props>,
{
    fn draw(&self, frame: &mut Frame, props: Props, metadata: DrawMetadata) {
        self.deref().draw(frame, props, metadata)
    }
}

/// Metadata associated with each draw action, which may instruct how the draw
/// should occur.
#[derive(Copy, Clone, Debug, Default)]
pub struct DrawMetadata {
    /// Which area on the screen should we draw to?
    area: Rect,
    /// Does the drawn component have focus? Focus indicates the component
    /// receives keyboard events. Most of the time, the focused element should
    /// get some visual indicator that it's in focus.
    has_focus: bool,
}

impl DrawMetadata {
    /// Construct a new metadata. The naming is chosen to discourage calling
    /// this directly, which in turn discourages calling [Draw::draw] correctly.
    /// Instead, use
    /// [Component::draw](crate::tui::view::component::Component::draw).
    ///
    /// It should probably be better to restrict this via visibility, but that
    /// requires refactoring the module layout and I'm not sure the benefit is
    /// worth it.
    pub fn new_dangerous(area: Rect, has_focus: bool) -> Self {
        Self { area, has_focus }
    }

    /// Which area on the screen should we draw to?
    pub fn area(self) -> Rect {
        self.area
    }

    /// Does the component have focus, i.e. is it the component that should
    /// receive keyboard events?
    pub fn has_focus(self) -> bool {
        self.has_focus
    }
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

/// Marker trait to pull in a blanket impl of [Generate], which simply calls
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
