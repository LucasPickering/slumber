mod help;
mod misc;
mod primary;
mod profile_select;
mod recipe_list;
mod recipe_pane;
mod record_body;
mod request_pane;
mod response_pane;
mod root;

pub use root::Root;

use crate::tui::view::{draw::Draw, event::EventHandler};
use crossterm::event::MouseEvent;
use ratatui::{layout::Rect, Frame};
use std::cell::Cell;

/// A wrapper around the various component types. The main job of this is to
/// automatically track the area that a component is drawn to, so that it can
/// be used during event handling to filter cursor events. This makes it easy
/// to have components automatically receive *only the cursor events* that
/// occurred within the bounds of that component. Generally every layer in the
/// component tree should be wrapped in one of these.
///
/// This intentionally does *not* implement `Deref` because that has led to bugs
/// in the past where the inner component is drawn without this pass through
/// layer, which means the component area isn't tracked. That means cursor
/// events aren't handled.
#[derive(Debug, Default)]
pub struct Component<T> {
    inner: T,
    /// The area that this component was last rendered to. In most cases this
    /// is updated automatically by calling `draw`, but in some scenarios (such
    /// as headless components) we may need to manually set this via
    /// [Self::set_area].
    area: Cell<Rect>,
}

impl<T> Component<T> {
    pub fn new(inner: T) -> Self {
        Self {
            inner,
            area: Cell::default(),
        }
    }

    /// Get the visual area that this component was last drawn to.
    pub fn area(&self) -> Rect {
        self.area.get()
    }

    /// Get a mutable reference to the inner value, but as a trait object.
    /// Useful for returning from `[EventHandler::children]`.
    pub fn as_child(&mut self) -> Component<&mut dyn EventHandler>
    where
        T: EventHandler,
    {
        Component {
            inner: &mut self.inner,
            area: self.area.clone(),
        }
    }

    /// Did the given mouse event occur over/on this component?
    pub fn intersects(&self, mouse_event: &MouseEvent) -> bool {
        self.area().intersects(Rect {
            x: mouse_event.column,
            y: mouse_event.row,
            width: 1,
            height: 1,
        })
    }

    /// Get a reference to the inner component. This should only be used to
    /// access the contained *data*. Drawing should be routed through the
    /// wrapping component.
    pub fn data(&self) -> &T {
        &self.inner
    }

    /// Get a mutable reference to the inner component. This should only be used
    /// to access the contained *data*. Drawing should be routed through the
    /// wrapping component.
    pub fn data_mut(&mut self) -> &mut T {
        &mut self.inner
    }

    /// Move the inner component out
    pub fn into_data(self) -> T {
        self.inner
    }
}

impl<T> From<T> for Component<T> {
    fn from(inner: T) -> Self {
        Self::new(inner)
    }
}

impl<P, T: Draw<P>> Draw<P> for Component<T> {
    fn draw(&self, frame: &mut Frame, props: P, area: Rect) {
        self.area.set(area); // Cache the visual area, for event handling
        self.inner.draw(frame, props, area);
    }
}
