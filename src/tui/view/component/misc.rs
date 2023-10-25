//! Miscellaneous components. They have specific purposes and therefore aren't
//! generic/utility, but don't fall into a clear category.

use crate::{
    template::Prompt,
    tui::{
        input::Action,
        view::{
            component::{
                modal::IntoModal, Component, Draw, Modal, UpdateOutcome,
                ViewMessage,
            },
            state::Notification,
            util::{layout, ButtonBrick, ToTui},
            Frame, RenderContext,
        },
    },
};
use derive_more::Display;
use itertools::Itertools;
use ratatui::{
    prelude::{Alignment, Constraint, Direction, Rect},
    widgets::{Paragraph, Wrap},
};
use std::fmt::Debug;
use tui_textarea::TextArea;

#[derive(Debug, Display)]
#[display(fmt = "ErrorModal")]
pub struct ErrorModal(anyhow::Error);

impl Modal for ErrorModal {
    fn title(&self) -> &str {
        "Error"
    }

    fn dimensions(&self) -> (Constraint, Constraint) {
        (Constraint::Percentage(60), Constraint::Percentage(20))
    }
}

impl Component for ErrorModal {
    fn update(&mut self, message: ViewMessage) -> UpdateOutcome {
        match message {
            // Extra close action
            ViewMessage::Input {
                action: Some(Action::Interact),
                ..
            } => UpdateOutcome::Propagate(ViewMessage::CloseModal),

            _ => UpdateOutcome::Propagate(message),
        }
    }
}

impl Draw for ErrorModal {
    fn draw(
        &self,
        context: &RenderContext,
        _: (),
        frame: &mut Frame,
        chunk: Rect,
    ) {
        let [content_chunk, footer_chunk] = layout(
            chunk,
            Direction::Vertical,
            [Constraint::Min(0), Constraint::Length(1)],
        );

        frame.render_widget(
            Paragraph::new(self.0.to_tui(context)).wrap(Wrap::default()),
            content_chunk,
        );

        // Prompt the user to get out of here
        frame.render_widget(
            Paragraph::new(
                ButtonBrick {
                    text: "OK",
                    is_highlighted: true,
                }
                .to_tui(context),
            )
            .alignment(Alignment::Center),
            footer_chunk,
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
#[derive(Debug, Display)]
#[display(fmt = "PromptModal")]
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
}

impl Component for PromptModal {
    fn update(&mut self, message: ViewMessage) -> UpdateOutcome {
        match message {
            // Submit
            ViewMessage::Input {
                action: Some(Action::Interact),
                ..
            } => {
                // Submission is handled in on_close. The control flow here is
                // ugly but it's hard with the top-down nature of modals
                self.submit = true;
                UpdateOutcome::Propagate(ViewMessage::CloseModal)
            }

            // All other input gets forwarded to the text editor (except cancel)
            ViewMessage::Input { event, action }
                if action != Some(Action::Cancel) =>
            {
                self.text_area.input(event);
                UpdateOutcome::Consumed
            }

            _ => UpdateOutcome::Propagate(message),
        }
    }
}

impl Draw for PromptModal {
    fn draw(
        &self,
        _context: &RenderContext,
        _: (),
        frame: &mut Frame,
        chunk: Rect,
    ) {
        frame.render_widget(self.text_area.widget(), chunk);
    }
}

impl IntoModal for Prompt {
    type Target = PromptModal;

    fn into_modal(self) -> Self::Target {
        PromptModal::new(self)
    }
}

#[derive(Debug)]
pub struct HelpText;

impl Draw for HelpText {
    fn draw(
        &self,
        context: &RenderContext,
        _: (),
        frame: &mut Frame,
        chunk: Rect,
    ) {
        let actions = [
            Action::Quit,
            Action::ReloadCollection,
            Action::FocusNext,
            Action::FocusPrevious,
            Action::Fullscreen,
            Action::Cancel,
        ];
        let text = actions
            .into_iter()
            .map(|action| {
                context
                    .input_engine
                    .binding(action)
                    .as_ref()
                    .map(ToString::to_string)
                    // This *shouldn't* happen, all actions get a binding
                    .unwrap_or_else(|| "???".into())
            })
            .join(" / ");
        frame.render_widget(Paragraph::new(text), chunk);
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
    fn draw(
        &self,
        context: &RenderContext,
        _: (),
        frame: &mut Frame,
        chunk: Rect,
    ) {
        frame.render_widget(
            Paragraph::new(self.notification.to_tui(context)),
            chunk,
        );
    }
}
