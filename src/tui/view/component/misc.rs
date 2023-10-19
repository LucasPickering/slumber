//! Miscellaneous components. They have specific purposes and therefore aren't
//! generic/utility, but don't fall into a clear category.

use crate::{
    template::Prompt,
    tui::{
        input::Action,
        view::{
            component::{Component, Draw, UpdateOutcome, ViewMessage},
            state::Notification,
            util::{layout, ButtonBrick, Modal, ModalContent, ToTui},
            Frame, RenderContext,
        },
    },
};
use derive_more::From;
use itertools::Itertools;
use ratatui::{
    prelude::{Alignment, Constraint, Direction, Rect},
    widgets::{Paragraph, Wrap},
};
use std::fmt::Debug;
use tui_textarea::TextArea;

/// A modal to show the user a catastrophic error
pub type ErrorModal = Modal<ErrorModalInner>;

impl Component for ErrorModal {
    fn update(&mut self, message: ViewMessage) -> UpdateOutcome {
        match message {
            // Open the modal
            ViewMessage::Error(error) => {
                self.open(error.into());
                UpdateOutcome::Consumed
            }

            // Close the modal
            ViewMessage::InputAction {
                action: Some(Action::Interact | Action::Close),
                ..
            } if self.is_open() => {
                self.close();
                UpdateOutcome::Consumed
            }

            _ => UpdateOutcome::Propagate(message),
        }
    }
}

#[derive(Debug, From)]
pub struct ErrorModalInner(anyhow::Error);

impl ModalContent for ErrorModalInner {
    fn title(&self) -> &str {
        "Error"
    }

    fn dimensions(&self) -> (Constraint, Constraint) {
        (Constraint::Percentage(60), Constraint::Percentage(20))
    }
}

impl Draw for ErrorModalInner {
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

/// A modal to prompt the user for some input
pub type PromptModal = Modal<PromptModalInner>;

impl Component for PromptModal {
    fn update(&mut self, message: ViewMessage) -> UpdateOutcome {
        match message {
            // Open the prompt
            ViewMessage::Prompt(prompt) => {
                // Listen for this outside the child, because it won't be in
                // focus while closed
                self.open(PromptModalInner::new(prompt));
                UpdateOutcome::Consumed
            }

            // Close
            ViewMessage::InputAction {
                action: Some(Action::Close),
                ..
            } if self.is_open() => {
                // Dropping the prompt returner here will tell the caller
                // that we're not returning anything
                self.close();
                UpdateOutcome::Consumed
            }

            // Submit
            ViewMessage::InputAction {
                action: Some(Action::Interact),
                ..
            } if self.is_open() => {
                // Return the user's value and close the prompt
                let inner = self.close().expect("We checked is_open");
                let input = inner.text_area.into_lines().join("\n");
                inner.prompt.respond(input);
                UpdateOutcome::Consumed
            }

            // All other input gets forwarded to the text editor
            ViewMessage::InputAction { event, .. } if self.is_open() => {
                let text_area = match self {
                    Modal::Closed => unreachable!("We checked is_open"),
                    Modal::Open(PromptModalInner { text_area, .. }) => {
                        text_area
                    }
                };
                text_area.input(event);
                UpdateOutcome::Consumed
            }

            _ => UpdateOutcome::Propagate(message),
        }
    }
}

/// Inner state for the prompt modal
#[derive(Debug)]
pub struct PromptModalInner {
    prompt: Prompt,
    text_area: TextArea<'static>,
}

impl PromptModalInner {
    pub fn new(prompt: Prompt) -> Self {
        let mut text_area = TextArea::default();
        if prompt.sensitive() {
            text_area.set_mask_char('\u{2022}');
        }
        Self { prompt, text_area }
    }
}

impl ModalContent for PromptModalInner {
    fn title(&self) -> &str {
        self.prompt.label()
    }

    fn dimensions(&self) -> (Constraint, Constraint) {
        (Constraint::Percentage(60), Constraint::Length(3))
    }
}

impl Draw for PromptModalInner {
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
            Action::Close,
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
