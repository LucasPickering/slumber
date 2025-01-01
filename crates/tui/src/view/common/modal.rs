use crate::{
    context::TuiContext,
    view::{
        context::UpdateContext,
        draw::{Draw, DrawMetadata},
        event::{Child, Emitter, EmitterHandle, Event, EventHandler, Update},
        util::centered_rect,
        Component, ViewContext,
    },
};
use ratatui::{
    prelude::Constraint,
    text::Line,
    widgets::{Block, Borders, Clear},
    Frame,
};
use slumber_config::Action;
use std::{collections::VecDeque, fmt::Debug, ops::DerefMut};
use tracing::trace;

/// A modal (AKA popup or dialog) is a high-priority element to be shown to the
/// user. It may be informational (e.g. an error message) or interactive (e.g.
/// an input prompt). Any type that implements this trait can be used as a
/// modal.
///
/// Modals cannot take props because they are rendered by the root component
/// with dynamic dispatch, and therefore all modals must take the same props
/// (none).
pub trait Modal: Debug + Draw<()> + EventHandler {
    /// Should this modal go to the front or back of the queue? Typically this
    /// is static for a particular implementation, but it's defined as a method
    /// for object-safetyability
    fn priority(&self) -> ModalPriority {
        ModalPriority::Low
    }

    /// Text at the top of the modal
    fn title(&self) -> Line<'_>;

    /// Dimensions of the modal, relative to the whole screen
    fn dimensions(&self) -> (Constraint, Constraint);

    /// Send an event to close this modal. `submitted` flag will be forwarded
    /// to the `on_close` handler.
    fn close(&self, submitted: bool) {
        ViewContext::push_event(Event::CloseModal { submitted });
    }

    /// Optional callback when the modal is closed. Useful for finishing
    /// operations that require ownership of the modal data. Submitted flag is
    /// set based on the correspond flag in the `CloseModal` event.
    fn on_close(self: Box<Self>, _submitted: bool) {}
}

impl EventHandler for Box<dyn Modal> {
    fn update(&mut self, context: &mut UpdateContext, event: Event) -> Update {
        self.deref_mut().update(context, event)
    }

    fn children(&mut self) -> Vec<Component<Child<'_>>> {
        self.deref_mut().children()
    }
}

/// Define how a type can be converted into a modal. Often times, implementors
/// of [Modal] will be esoteric types that external consumers who want to open
/// a modal aren't concerned about. This trait provides an adapter layer
/// between the type a user might have (e.g. [anyhow::Error]) and the inner
/// modal type (e.g. `ErrorModal`). Inspired by `Iterator` and `IntoIterator`.
pub trait IntoModal {
    type Target: Modal;

    fn into_modal(self) -> Self::Target;
}

/// A singleton component to hold all modals at the root of the tree, so that
/// they render on top.
#[derive(Debug, Default)]
pub struct ModalQueue {
    queue: VecDeque<Component<Box<dyn Modal>>>,
}

/// Priority defines where in the modal queue to add a new modal. Most modals
/// should be low priority, but things like errors should be high priority.
#[derive(Debug, Default)]
pub enum ModalPriority {
    /// Open modal at the back of the queue
    #[default]
    Low,
    /// Open modal at the front of the queue
    High,
}

impl ModalQueue {
    /// Is there a modal open right now?
    pub fn is_open(&self) -> bool {
        !self.queue.is_empty()
    }

    /// Add a new modal, to either the beginning or end of the queue, depending
    /// on priority
    pub fn open(&mut self, modal: Box<dyn Modal>) {
        trace!(?modal, "Opening modal");
        match modal.priority() {
            ModalPriority::Low => {
                self.queue.push_back(modal.into());
            }
            ModalPriority::High => {
                self.queue.push_front(modal.into());
            }
        }
    }

    /// Close the current modal
    fn close(&mut self, submitted: bool) {
        trace!("Closing modal");
        if let Some(modal) = self.queue.pop_front().map(Component::into_data) {
            modal.on_close(submitted);
        }
    }
}

impl EventHandler for ModalQueue {
    fn update(&mut self, _: &mut UpdateContext, event: Event) -> Update {
        match event {
            // Close the active modal. If there's no modal open, we'll propagate
            // the event down
            Event::Input {
                // Enter to close is a convenience thing, modals may override.
                // We eat the Quit action here because it's (hopefully)
                // intuitive and consistent with other TUIs
                action:
                    Some(
                        action @ (Action::Cancel
                        | Action::Quit
                        | Action::Submit),
                    ),
                event: _,
            } if self.is_open() => {
                self.close(action == Action::Submit);
            }
            Event::CloseModal { submitted } if self.is_open() => {
                self.close(submitted);
            }

            // If open, eat all cursor events so they don't get sent to
            // background components
            Event::Input {
                action: _,
                event: crossterm::event::Event::Mouse(_),
            } if self.is_open() => {}

            // Open a new modal
            Event::OpenModal(modal) => self.open(modal),

            _ => return Update::Propagate(event),
        }
        Update::Consumed
    }

    fn children(&mut self) -> Vec<Component<Child<'_>>> {
        self.queue
            .front_mut()
            .map(Component::to_child_mut)
            .into_iter()
            .collect()
    }
}

impl Draw for ModalQueue {
    fn draw(&self, frame: &mut Frame, _: (), metadata: DrawMetadata) {
        if let Some(modal) = self.queue.front() {
            let styles = &TuiContext::get().styles;
            let (width, height) = modal.data().dimensions();

            // The child gave us the content dimensions, we need to add one cell
            // of buffer for the border
            let mut area = centered_rect(width, height, metadata.area());
            area.x -= 1;
            area.y -= 1;
            area.width += 2;
            area.height += 2;

            let block = Block::default()
                .title(modal.data().title())
                .borders(Borders::ALL)
                .border_style(styles.modal.border)
                .border_type(styles.modal.border_type);
            let inner_area = block.inner(area);

            // Draw the outline of the modal
            frame.render_widget(Clear, area);
            frame.render_widget(block, area);

            // Render the actual content
            modal.draw(frame, (), inner_area, true);
        }
    }
}

/// A helper to manage opened modals. Useful for components that need to open
/// a modal of a particular type, then listen for emitted events from that
/// modal. This only supports **a single modal at a time** of that type.
#[derive(Debug)]
pub struct ModalHandle<T: Emitter> {
    /// Track the emitter ID of the opened modal, so we can check for emitted
    /// events from it later. This is `None` on initialization. Note: this does
    /// *not* get cleared when a modal is closed, because that requires extra
    /// plumbing but would not actually accomplish anything. Once a modal is
    /// closed, it won't be emitting anymore so there's no harm in hanging onto
    /// its ID.
    emitter: Option<EmitterHandle<T::Emitted>>,
}

impl<T: Emitter> ModalHandle<T> {
    pub fn new() -> Self {
        Self { emitter: None }
    }

    /// Open a modal and store its emitter ID
    pub fn open(&mut self, modal: T)
    where
        T: 'static + Modal,
    {
        self.emitter = Some(modal.handle());
        ViewContext::open_modal(modal);
    }

    /// Check if an event was emitted by the most recently opened modal
    pub fn emitted<'a>(&self, event: &'a Event) -> Option<&'a T::Emitted> {
        self.emitter
            .as_ref()
            .and_then(|emitter| emitter.emitted(event))
    }
}

impl<T: Emitter> Default for ModalHandle<T> {
    fn default() -> Self {
        Self { emitter: None }
    }
}
