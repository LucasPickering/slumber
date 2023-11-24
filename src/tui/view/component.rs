//! Specific single-use components

pub mod help;
pub mod misc;
pub mod primary;
pub mod request;
pub mod response;
pub mod root;
pub mod settings;

use crate::tui::view::{
    draw::{Draw, DrawContext},
    event::{Event, EventHandler, Update, UpdateContext},
};
use crossterm::event::MouseEvent;
use derive_more::{Deref, DerefMut};
pub use primary::FullscreenMode;
use ratatui::layout::Rect;
pub use root::Root;
use std::cell::Cell;

/// A wrapper around the various component types. The main job of this is to
/// automatically track the area that a component is drawn to, so that it can
/// be used during event handling to filter cursor events. This makes it easy
/// to have components automatically receive *only the cursor events* that
/// occurred within the bounds of that component. Generally every layer in the
/// component tree should be wrapped in one of these.
#[derive(Debug, Default, Deref, DerefMut)]
pub struct Component<T> {
    #[deref]
    #[deref_mut]
    inner: T,
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

    /// Manually set the area on this component. In most cases you don't need
    /// to call this, because the area is automatically set in `[Self::draw]`.
    /// But for components that aren't drawn (i.e. state-only components), we
    /// may need to manually capture the area so we can still handle mouse
    /// events.
    ///
    /// This isn't a great pattern, but it's easy and works for now.
    pub fn set_area(&self, area: Rect) {
        self.area.replace(area);
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

    /// Move the inner component out
    pub fn into_inner(self) -> T {
        self.inner
    }
}

impl<T: EventHandler> EventHandler for Component<T> {
    fn update(&mut self, context: &mut UpdateContext, event: Event) -> Update {
        self.inner.update(context, event)
    }

    fn children(&mut self) -> Vec<Component<&mut dyn EventHandler>> {
        self.inner.children()
    }
}

impl<P, T: Draw<P>> Draw<P> for Component<T> {
    fn draw(&self, context: &mut DrawContext, props: P, area: Rect) {
        self.area.set(area); // Cache the visual area, for event handling
        self.inner.draw(context, props, area);
    }
}

impl<'a, P, T> Draw<P> for &'a Component<T>
where
    &'a T: Draw<P>,
{
    fn draw(&self, context: &mut DrawContext, props: P, area: Rect) {
        self.area.set(area); // Cache the visual area, for event handling
        (&self.inner).draw(context, props, area);
    }
}

impl<T> From<T> for Component<T> {
    fn from(inner: T) -> Self {
        Self::new(inner)
    }
}
