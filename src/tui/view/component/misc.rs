//! Miscellaneous components. They have specific purposes and therefore aren't
//! generic/utility, but don't fall into a clear category.

use crate::{
    template::Prompt,
    tui::{
        input::Action,
        view::{
            component::{
                modal::IntoModal, primary::PrimaryPane, root::FullscreenMode,
                Component, Draw, Event, Modal, Update, UpdateContext,
            },
            state::Notification,
            util::{layout, ButtonBrick, ToTui},
            DrawContext,
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
    fn update(&mut self, context: &mut UpdateContext, event: Event) -> Update {
        match event {
            // Extra close action
            Event::Input {
                action: Some(Action::Submit),
                ..
            } => {
                context.queue_event(Event::CloseModal);
                Update::Consumed
            }

            _ => Update::Propagate(event),
        }
    }
}

impl Draw for ErrorModal {
    fn draw(&self, context: &mut DrawContext, _: (), chunk: Rect) {
        let [content_chunk, footer_chunk] = layout(
            chunk,
            Direction::Vertical,
            [Constraint::Min(0), Constraint::Length(1)],
        );

        context.frame.render_widget(
            Paragraph::new(self.0.to_tui(context)).wrap(Wrap::default()),
            content_chunk,
        );

        // Prompt the user to get out of here
        context.frame.render_widget(
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

/// Inner state forfn update(&mut self, context:&mut UpdateContext, message:
/// Event) -> UpdateOutcome the prompt modal
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

/// Tell the user about keybindings
#[derive(Debug)]
pub struct HelpText;

pub struct HelpTextProps {
    pub has_modal: bool,
    pub fullscreen_mode: Option<FullscreenMode>,
    pub selected_pane: PrimaryPane,
}

impl Draw<HelpTextProps> for HelpText {
    fn draw(
        &self,
        context: &mut DrawContext,
        props: HelpTextProps,
        chunk: Rect,
    ) {
        // Decide which actions to show based on context. This is definitely
        // spaghetti and easy to get out of sync, but it's the easiest way to
        // get granular control
        let mut actions = vec![Action::Quit];

        // Modal overrides everything else
        if props.has_modal {
            actions.push(Action::Cancel);
        } else {
            match props.fullscreen_mode {
                None => {
                    actions.extend([
                        Action::ReloadCollection,
                        Action::SendRequest,
                        Action::NextPane,
                        Action::PreviousPane,
                        Action::OpenSettings,
                    ]);
                    // Pane-specific actions
                    actions.extend(match props.selected_pane {
                        PrimaryPane::ProfileList => &[] as &[Action],
                        PrimaryPane::RecipeList => &[],
                        PrimaryPane::Request => &[Action::Fullscreen],
                        PrimaryPane::Response => &[Action::Fullscreen],
                    });
                }
                Some(_) => actions.extend([Action::Fullscreen]),
            }
        }

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
        context.frame.render_widget(Paragraph::new(text), chunk);
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
        context.frame.render_widget(
            Paragraph::new(self.notification.to_tui(context)),
            chunk,
        );
    }
}
