use crate::{
    context::TuiContext,
    view::{
        Component, UpdateContext,
        component::{Canvas, Child, ComponentId, Draw, DrawMetadata, Portal},
        event::{Emitter, Event, LocalEvent, OptionEvent, ToEmitter},
        util::centered_rect,
    },
};
use ratatui::{
    layout::{Constraint, Margin, Rect},
    text::Line,
    widgets::{Block, Borders},
};
use slumber_config::Action;
use std::{collections::VecDeque, fmt::Debug};

/// A *homogenous* queue of modals. TODO
#[derive(Debug)]
pub struct ModalQueue<T> {
    id: ComponentId,
    /// All the queued modals. The front modal will be visible, the rest are
    /// patiently waiting their turn.
    queue: VecDeque<T>,
}

// Remove bound on T
impl<T> Default for ModalQueue<T> {
    fn default() -> Self {
        Self {
            id: ComponentId::default(),
            queue: VecDeque::default(),
        }
    }
}

impl<T: Modal> ModalQueue<T> {
    /// Is there a modal visible?
    pub fn is_open(&self) -> bool {
        !self.queue.is_empty()
    }

    /// Add a new modal to the back of the queue. If the queue is empty, it will
    /// be displayed immediately.
    pub fn open(&mut self, modal: T) {
        self.queue.push_back(modal);
    }

    /// Close the visible modal at the front of the queue. If `submitted` is
    /// `true`, call [Modal::on_submit] for the closed modal.
    pub fn close(&mut self, context: &mut UpdateContext, submitted: bool) {
        let popped = self.queue.pop_front();
        if let Some(modal) = popped
            && submitted
        {
            modal.on_submit(context);
        }
    }

    fn active(&self) -> Option<&T> {
        self.queue.front()
    }

    fn active_mut(&mut self) -> Option<&mut T> {
        self.queue.front_mut()
    }
}

// Through the power of PORTALS, we can always render modals on top. The Modal
// impl tells us how big the modal content will be, so we can report how much
// space we intend to take up in the middle of the screen.
impl<T: Modal> Portal for ModalQueue<T> {
    fn area(&self, canvas_area: Rect) -> Rect {
        fn pad(constraint: Constraint, padding: u16) -> Constraint {
            match constraint {
                Constraint::Min(l) => Constraint::Min(l + padding),
                Constraint::Max(l) => Constraint::Max(l + padding),
                Constraint::Length(l) => Constraint::Length(l + padding),
                Constraint::Percentage(_)
                | Constraint::Ratio(_, _)
                | Constraint::Fill(_) => constraint,
            }
        }

        if let Some(modal) = self.active() {
            let (width, height) = modal.dimensions();
            // Add space for the modal border/padding to fixed-length
            // constraints. Percentages will be unaffected
            centered_rect(pad(width, 4), pad(height, 2), canvas_area)
        } else {
            Rect::default()
        }
    }
}

impl<T: Component + Modal> Component for ModalQueue<T> {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn update(
        &mut self,
        context: &mut UpdateContext,
        event: Event,
    ) -> Option<Event> {
        // If we're closed, don't eat any events
        if !self.is_open() {
            return Some(event);
        }

        event
            .opt()
            .action(|action, propagate| match action {
                // Close the active modal. If there's no modal open, we'll
                // propagate the event down. Enter to close is a convenience
                // thing, modals may override. We eat the Quit action here
                // because it's (hopefully) intuitive and consistent with other
                // TUIs
                Action::Cancel | Action::Quit => self.close(context, false),
                Action::Submit => self.close(context, true),
                _ => propagate.set(),
            })
            .emitted_opt(
                // If this modal type emits ModalEvent, check for that
                self.active().and_then(Modal::emitter),
                |event: ModalEvent| match event {
                    ModalEvent::Submit => self.close(context, true),
                },
            )
            .any(|event| match event {
                // Modals are meant to consume all focus, so don't allow any
                // events to go to background components
                Event::Input { .. } => None,
                _ => Some(event),
            })
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        if let Some(modal) = self.active_mut() {
            vec![modal.to_child_mut()]
        } else {
            vec![]
        }
    }
}

impl<T, Props> Draw<Props> for ModalQueue<T>
where
    T: Component + Draw<Props> + Modal,
{
    fn draw(&self, canvas: &mut Canvas, props: Props, metadata: DrawMetadata) {
        let Some(modal) = self.active() else {
            // No modals open - get the fuck outta here!!
            return;
        };

        let styles = &TuiContext::get().styles.modal;
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(styles.border)
            .border_type(styles.border_type)
            .title(modal.title());
        // Add one cell of X padding so text doesn't butt up against the border;
        // that would interfere with word-based selection
        let margin = Margin::new(1, 0);

        canvas.render_widget(&block, metadata.area());
        let area = block.inner(metadata.area()).inner(margin);
        canvas.draw(modal, props, area, true);
    }
}

// TODO this good? bad?
impl<T: ToEmitter<E>, E: LocalEvent> ToEmitter<E> for ModalQueue<T> {
    fn to_emitter(&self) -> Emitter<E> {
        self.queue
            .front()
            .map(ToEmitter::to_emitter)
            .unwrap_or(Emitter::null())
    }
}

/// A modal (AKA popup or dialog) is a high-priority element to be shown to the
/// user. It may be informational (e.g. an error message) or interactive (e.g.
/// an input prompt). Any type that implements this trait can be used as a
/// modal.
///
/// Modals cannot take props because they are rendered by the root component
/// with dynamic dispatch, and therefore all modals must take the same props
/// (none).
pub trait Modal {
    /// Text at the top of the modal
    fn title(&self) -> Line<'_>;

    /// Dimensions of the modal, relative to the whole screen
    fn dimensions(&self) -> (Constraint, Constraint);

    /// TODO
    fn emitter(&self) -> Option<Emitter<ModalEvent>> {
        None
    }

    /// TODO
    fn on_submit(self, _context: &mut UpdateContext)
    where
        Self: Sized,
    {
    }
}

/// An emitted event that allows a modal to communicate back to its containing
/// [ModalQueue].
#[derive(Debug)]
pub enum ModalEvent {
    /// Close the modal *with* submission
    Submit,
}
