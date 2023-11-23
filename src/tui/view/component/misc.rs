//! Miscellaneous components. They have specific purposes and therefore aren't
//! generic/utility, but don't fall into a clear category.

use crate::{
    template::Prompt,
    tui::{
        input::Action,
        view::{
            common::modal::{IntoModal, Modal},
            draw::{Draw, DrawContext, Generate},
            event::{Event, EventHandler, Update, UpdateContext},
            state::Notification,
        },
    },
};
use ratatui::{
    prelude::{Constraint, Rect},
    widgets::{Paragraph, Wrap},
};
use std::fmt::Debug;
use tui_textarea::TextArea;

#[derive(Debug)]
pub struct ErrorModal(anyhow::Error);

impl Modal for ErrorModal {
    fn title(&self) -> &str {
        "Error"
    }

    fn dimensions(&self) -> (Constraint, Constraint) {
        (Constraint::Percentage(60), Constraint::Percentage(20))
    }

    fn as_event_handler(&mut self) -> &mut dyn EventHandler {
        self
    }
}

impl EventHandler for ErrorModal {}

impl Draw for ErrorModal {
    fn draw(&self, context: &mut DrawContext, _: (), chunk: Rect) {
        context.frame.render_widget(
            Paragraph::new(self.0.generate()).wrap(Wrap::default()),
            chunk,
        );
    }
}

impl IntoModal for anyhow::Error {
    type Target = ErrorModal;

    fn into_modal(self) -> Self::Target {
        ErrorModal(self)
    }
}

/// Inner state forfn update(&mut self, context:&mut UpdateContext, message:
/// Event) -> UpdateOutcome the prompt modal
#[derive(Debug)]
pub struct PromptModal {
    /// Prompt currently being shown
    prompt: Prompt,
    /// A queue of additional prompts to shown. If the queue is populated,
    /// closing one prompt will open a the next one.
    text_area: TextArea<'static>,
    /// Flag set before closing to indicate if we should submit in `on_close``
    submit: bool,
}

impl PromptModal {
    pub fn new(prompt: Prompt) -> Self {
        let mut text_area = TextArea::default();
        if prompt.sensitive() {
            text_area.set_mask_char('\u{2022}');
        }
        Self {
            prompt,
            text_area,
            submit: false,
        }
    }
}

impl Modal for PromptModal {
    fn title(&self) -> &str {
        self.prompt.label()
    }

    fn dimensions(&self) -> (Constraint, Constraint) {
        (Constraint::Percentage(60), Constraint::Length(3))
    }

    fn on_close(self: Box<Self>) {
        if self.submit {
            // Return the user's value and close the prompt
            let input = self.text_area.into_lines().join("\n");
            self.prompt.respond(input);
        }
    }

    fn as_event_handler(&mut self) -> &mut dyn EventHandler {
        self
    }
}

impl EventHandler for PromptModal {
    fn update(&mut self, context: &mut UpdateContext, event: Event) -> Update {
        match event {
            // Submit
            Event::Input {
                action: Some(Action::Submit),
                ..
            } => {
                // Submission is handled in on_close. The control flow here is
                // ugly but it's hard with the top-down nature of modals
                self.submit = true;
                context.queue_event(Event::CloseModal);
                Update::Consumed
            }

            // Make sure cancel gets propagated to close the modal
            event @ Event::Input {
                action: Some(Action::Cancel),
                ..
            } => Update::Propagate(event),

            // All other input gets forwarded to the text editor
            Event::Input { event, .. } => {
                self.text_area.input(event);
                Update::Consumed
            }

            _ => Update::Propagate(event),
        }
    }
}

impl Draw for PromptModal {
    fn draw(&self, context: &mut DrawContext, _: (), chunk: Rect) {
        context.frame.render_widget(self.text_area.widget(), chunk);
    }
}

impl IntoModal for Prompt {
    type Target = PromptModal;

    fn into_modal(self) -> Self::Target {
        PromptModal::new(self)
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
    fn draw(&self, context: &mut DrawContext, _: (), chunk: Rect) {
        context
            .frame
            .render_widget(Paragraph::new(self.notification.generate()), chunk);
    }
}
