use crate::tui::{
    input::Action,
    view::{
        component::{Component, Draw, UpdateOutcome, ViewMessage},
        util::centered_rect,
    },
};
use ratatui::{
    prelude::Constraint,
    widgets::{Block, BorderType, Borders, Clear},
};
use std::{collections::VecDeque, ops::DerefMut};
use tracing::trace;

/// A modal (AKA popup or dialog) is a high-priority element to be shown to the
/// user. It may be informational (e.g. an error message) or interactive (e.g.
/// an input prompt). Any type that implements this trait can be used as a
/// modal.
pub trait Modal: Draw<()> + Component {
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

#[derive(Debug)]
pub struct ModalQueue {
    queue: VecDeque<Box<dyn Modal>>,
}

impl ModalQueue {
    pub fn new() -> Self {
        Self {
            queue: VecDeque::new(),
        }
    }

    /// Add a new modal to the end of the queue
    pub fn open(&mut self, modal: Box<dyn Modal>) {
        trace!("Opening modal");
        self.queue.push_back(modal);
    }

    /// Close the current modal, and return the closed modal if any
    pub fn close(&mut self) -> Option<Box<dyn Modal>> {
        trace!("Closing modal");
        self.queue.pop_front()
    }
}

impl Component for ModalQueue {
    fn update(&mut self, message: ViewMessage) -> UpdateOutcome {
        match message {
            // Close the active modal
            ViewMessage::InputAction {
                action: Some(Action::Cancel),
                ..
            }
            | ViewMessage::CloseModal => match self.close() {
                Some(modal) => {
                    // Inform the modal of its terminal status
                    modal.on_close();
                    UpdateOutcome::Consumed
                }
                // Modal wasn't open, so don't consume the event
                None => UpdateOutcome::Propagate(message),
            },

            // Open a new modal
            ViewMessage::OpenModal(modal) => {
                self.open(modal);
                UpdateOutcome::Consumed
            }

            _ => UpdateOutcome::Propagate(message),
        }
    }

    fn focused_children(&mut self) -> Vec<&mut dyn Component> {
        match self.queue.front_mut() {
            Some(first) => vec![first.deref_mut()],
            None => vec![],
        }
    }
}

impl Draw for ModalQueue {
    fn draw(
        &self,
        context: &crate::tui::view::RenderContext,
        _: (),
        frame: &mut crate::tui::view::Frame,
        chunk: ratatui::prelude::Rect,
    ) {
        if let Some(modal) = self.queue.front() {
            let (x, y) = modal.dimensions();
            let chunk = centered_rect(x, y, chunk);
            let block = Block::default()
                .title(modal.title())
                .borders(Borders::ALL)
                .border_type(BorderType::Thick);
            let inner_chunk = block.inner(chunk);

            // Draw the outline of the modal
            frame.render_widget(Clear, chunk);
            frame.render_widget(block, chunk);

            // Render the actual content
            modal.draw(context, (), frame, inner_chunk);
        }
    }
}
