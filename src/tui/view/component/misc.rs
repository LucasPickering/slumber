//! Miscellaneous components. They have specific purposes and therefore aren't
//! generic/utility, but don't fall into a clear category.

use crate::{
    template::Prompt,
    tui::view::{
        common::{
            modal::{IntoModal, Modal},
            text_box::TextBox,
        },
        component::Component,
        draw::{Draw, Generate},
        event::{Event, EventHandler},
        state::Notification,
    },
};
use ratatui::{
    prelude::{Constraint, Rect},
    widgets::{Paragraph, Wrap},
    Frame,
};
use std::{cell::Cell, fmt::Debug, rc::Rc};

#[derive(Debug)]
pub struct ErrorModal(anyhow::Error);

impl Modal for ErrorModal {
    fn title(&self) -> &str {
        "Error"
    }

    fn dimensions(&self) -> (Constraint, Constraint) {
        (Constraint::Percentage(60), Constraint::Percentage(20))
    }
}

impl EventHandler for ErrorModal {}

impl Draw for ErrorModal {
    fn draw(&self, frame: &mut Frame, _: (), area: Rect) {
        frame.render_widget(
            Paragraph::new(self.0.generate()).wrap(Wrap::default()),
            area,
        );
    }
}

impl IntoModal for anyhow::Error {
    type Target = ErrorModal;

    fn into_modal(self) -> Self::Target {
        ErrorModal(self)
    }
}

/// Inner state for the prompt modal
#[derive(Debug)]
pub struct PromptModal {
    /// Prompt currently being shown
    prompt: Prompt,
    /// Flag set before closing to indicate if we should submit in our own
    /// `on_close`. This is set from the text box's `on_submit`.
    submit: Rc<Cell<bool>>,
    /// Little editor fucker
    text_box: Component<TextBox>,
}

impl PromptModal {
    pub fn new(prompt: Prompt) -> Self {
        let submit = Rc::new(Cell::new(false));
        let submit_cell = Rc::clone(&submit);
        let text_box = TextBox::default()
            .sensitive(true)
            // Make sure cancel gets propagated to close the modal
            .on_cancel(|_, context| context.queue_event(Event::CloseModal))
            .on_submit(move |_, context| {
                // We have to defer submission to on_close, because we need the
                // owned value of `self.prompt`. We could have just put that in
                // a refcell, but this felt a bit cleaner because we know this
                // submitter will only be called once.
                submit_cell.set(true);
                context.queue_event(Event::CloseModal);
            })
            .into();
        Self {
            prompt,
            submit,
            text_box,
        }
    }
}

impl Modal for PromptModal {
    fn title(&self) -> &str {
        self.prompt.label()
    }

    fn dimensions(&self) -> (Constraint, Constraint) {
        (Constraint::Percentage(60), Constraint::Length(1))
    }

    fn on_close(self: Box<Self>) {
        if self.submit.get() {
            // Return the user's value and close the prompt
            self.prompt.respond(self.text_box.into_inner().into_text());
        }
    }
}

impl EventHandler for PromptModal {
    fn children(&mut self) -> Vec<Component<&mut dyn EventHandler>> {
        vec![self.text_box.as_child()]
    }
}

impl Draw for PromptModal {
    fn draw(&self, frame: &mut Frame, _: (), area: Rect) {
        self.text_box.draw(frame, (), area);
    }
}

impl IntoModal for Prompt {
    type Target = PromptModal;

    fn into_modal(self) -> Self::Target {
        PromptModal::new(self)
    }
}

/// An empty actions modal, to show when no actions are available
#[derive(Debug, Default)]
pub struct EmptyActionsModal;

impl Modal for EmptyActionsModal {
    fn title(&self) -> &str {
        "Actions"
    }

    fn dimensions(&self) -> (Constraint, Constraint) {
        (Constraint::Length(30), Constraint::Length(1))
    }
}

impl EventHandler for EmptyActionsModal {}

impl Draw for EmptyActionsModal {
    fn draw(&self, frame: &mut Frame, _: (), area: Rect) {
        frame.render_widget(Paragraph::new("No actions available"), area);
    }
}

#[derive(Debug)]
pub struct NotificationText {
    notification: Notification,
}

impl NotificationText {
    pub fn new(notification: Notification) -> Self {
        Self { notification }
    }
}

impl Draw for NotificationText {
    fn draw(&self, frame: &mut Frame, _: (), area: Rect) {
        frame.render_widget(Paragraph::new(self.notification.generate()), area);
    }
}
