use crate::tui::{
    input::Action,
    view::{
        draw::{Draw, DrawContext},
        event::{Event, EventHandler, Update, UpdateContext},
        util::centered_rect,
        Component,
    },
};
use ratatui::{
    prelude::{Constraint, Rect},
    widgets::{Block, BorderType, Borders, Clear},
};
use std::{collections::VecDeque, ops::DerefMut};
use tracing::trace;

/// A modal (AKA popup or dialog) is a high-priority element to be shown to the
/// user. It may be informational (e.g. an error message) or interactive (e.g.
/// an input prompt). Any type that implements this trait can be used as a
/// modal.
///
/// Modals cannot take props because they are rendered by the root component
/// with dynamic dispatch, and therefore all modals must take the same props
/// (none).
pub trait Modal: Draw<()> + EventHandler {
    /// Text at the top of the modal
    fn title(&self) -> &str;

    /// Dimensions of the modal, relative to the whole screen
    fn dimensions(&self) -> (Constraint, Constraint);

    /// Optional callback when the modal is closed. Useful for finishing
    /// operations that require ownership of the modal data.
    fn on_close(self: Box<Self>) {}
}

/// Define how a type can be converted into a modal. Often times, implementors
/// of [Modal] will be esoteric types that external consumers who want to open
/// a modal aren't concerned about. This trait provides an adapater layer
/// between the type a user might have (e.g. [anyhow::Error]) and the inner
/// modal type (e.g. [ErrorModal]). Inspired by `Iterator` and `IntoIterator`.
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
    pub fn open(&mut self, modal: Box<dyn Modal>, priority: ModalPriority) {
        trace!(?priority, "Opening modal");
        match priority {
            ModalPriority::Low => {
                self.queue.push_back(modal.into());
            }
            ModalPriority::High => {
                self.queue.push_front(modal.into());
            }
        }
    }

    /// Close the current modal, and return the closed modal if any
    pub fn close(&mut self) -> Option<Box<dyn Modal>> {
        trace!("Closing modal");
        self.queue.pop_front().map(Component::into_inner)
    }
}

impl EventHandler for ModalQueue {
    fn update(&mut self, _context: &mut UpdateContext, event: Event) -> Update {
        match event {
            // Close the active modal. If there's no modal open, we'll propagate
            // the event down
            Event::Input {
                // Enter to close is a convenience thing, modals may override
                action: Some(Action::Cancel | Action::Submit),
                ..
            }
            | Event::CloseModal => {
                match self.close() {
                    Some(modal) => {
                        // Inform the modal of its terminal status
                        modal.on_close();
                        Update::Consumed
                    }
                    // Modal wasn't open, so don't consume the event
                    None => Update::Propagate(event),
                }
            }

            // Open a new modal
            Event::OpenModal { modal, priority } => {
                self.open(modal, priority);
                Update::Consumed
            }

            _ => Update::Propagate(event),
        }
    }

    fn children(&mut self) -> Vec<Component<&mut dyn EventHandler>> {
        match self.queue.front_mut() {
            Some(first) => vec![first.as_child()],
            None => vec![],
        }
    }
}

impl Draw for ModalQueue {
    fn draw(&self, context: &mut DrawContext, _: (), area: Rect) {
        if let Some(modal) = self.queue.front() {
            let (width, height) = modal.dimensions();

            // The child gave us the content dimensions, we need to add one cell
            // of buffer for the border
            let mut area = centered_rect(width, height, area);
            area.x -= 1;
            area.y -= 1;
            area.width += 2;
            area.height += 2;

            let block = Block::default()
                .title(modal.title())
                .borders(Borders::ALL)
                .border_type(BorderType::Thick);
            let inner_area = block.inner(area);

            // Draw the outline of the modal
            context.frame.render_widget(Clear, area);
            context.frame.render_widget(block, area);

            // Render the actual content
            modal.draw(context, (), inner_area);
        }
    }
}

impl EventHandler for Box<dyn Modal> {
    fn update(&mut self, context: &mut UpdateContext, event: Event) -> Update {
        self.deref_mut().update(context, event)
    }

    fn children(&mut self) -> Vec<Component<&mut dyn EventHandler>> {
        self.deref_mut().children()
    }
}
