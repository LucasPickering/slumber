//! Miscellaneous components. They have specific purposes and therefore aren't
//! generic/utility, but don't fall into a clear category.

use crate::view::{
    Generate, Question,
    common::{
        button::ButtonGroup,
        modal::Modal,
        text_box::{TextBox, TextBoxProps},
    },
    component::{
        Canvas, Component, ComponentId, Draw, DrawMetadata,
        internal::{Child, ToChild},
    },
    context::UpdateContext,
};
use derive_more::Display;
use ratatui::{prelude::Constraint, text::Line};
use std::fmt::Debug;
use strum::{EnumCount, EnumIter};
use unicode_width::UnicodeWidthStr;

/// Modal to display an error. Typically the error is [anyhow::Error], but it
/// could also be wrapped in a smart pointer.
#[derive(Debug)]
pub struct ErrorModal {
    id: ComponentId,
    error: anyhow::Error,
}

impl ErrorModal {
    pub fn new(error: anyhow::Error) -> Self {
        ErrorModal {
            id: ComponentId::default(),
            error,
        }
    }
}

impl Modal for ErrorModal {
    fn title(&self) -> Line<'_> {
        "Error".into()
    }

    fn dimensions(&self) -> (Constraint, Constraint) {
        (Constraint::Percentage(60), Constraint::Percentage(20))
    }
}

impl Component for ErrorModal {
    fn id(&self) -> ComponentId {
        self.id
    }
}

impl Draw for ErrorModal {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        canvas.render_widget(self.error.generate(), metadata.area());
    }
}

/// Buttons in a yes/no confirmation modal
#[derive(
    Copy, Clone, Debug, Default, Display, EnumCount, EnumIter, PartialEq,
)]
pub enum ConfirmButton {
    No,
    #[default]
    Yes,
}

impl ConfirmButton {
    pub fn to_bool(self) -> bool {
        match self {
            ConfirmButton::No => false,
            ConfirmButton::Yes => true,
        }
    }
}

/// A modal to pose a question to the user
#[derive(derive_more::Debug)]
pub enum QuestionModal {
    /// Yes/no question
    Confirm {
        id: ComponentId,
        message: String,
        buttons: ButtonGroup<ConfirmButton>,
        /// Callback when the user replies
        #[debug(skip)]
        on_submit: Box<dyn 'static + FnOnce(bool)>,
    },

    /// Free-form text response
    Text {
        id: ComponentId,
        message: String,
        text_box: TextBox,
        /// Callback when the user replies
        #[debug(skip)]
        on_submit: Box<dyn 'static + FnOnce(String)>,
    },
}

impl QuestionModal {
    /// Open a modal with a yes/no question
    pub fn confirm(
        message: String,
        on_submit: impl 'static + FnOnce(bool),
    ) -> Self {
        Self::Confirm {
            id: ComponentId::new(),
            message,
            buttons: ButtonGroup::default(),
            on_submit: Box::new(on_submit),
        }
    }

    /// Open a modal to ask a question and get a text reply
    pub fn text(
        message: String,
        default: Option<String>,
        on_submit: impl 'static + FnOnce(String),
    ) -> Self {
        Self::Text {
            id: ComponentId::new(),
            message,
            text_box: TextBox::default()
                .default_value(default.unwrap_or_default()),
            on_submit: Box::new(on_submit),
        }
    }

    /// Build a new modal to ask a [Question]
    pub fn from_question(question: Question) -> Self {
        match question {
            Question::Confirm { message, channel } => {
                Self::confirm(message, move |reply| channel.reply(reply))
            }
            Question::Text {
                message,
                default,
                channel,
            } => {
                Self::text(message, default, move |reply| channel.reply(reply))
            }
        }
    }
}

impl Modal for QuestionModal {
    fn title(&self) -> Line<'_> {
        match self {
            QuestionModal::Confirm { message, .. }
            | QuestionModal::Text { message, .. } => message.as_str().into(),
        }
    }

    fn dimensions(&self) -> (Constraint, Constraint) {
        let width = match self {
            QuestionModal::Confirm { message, .. } => {
                // Add some arbitrary padding and a min width
                Constraint::Length((message.width() as u16 + 4).max(24))
            }
            QuestionModal::Text { .. } => Constraint::Percentage(60),
        };
        (width, Constraint::Length(1))
    }

    fn on_submit(self, _: &mut UpdateContext) {
        match self {
            QuestionModal::Confirm {
                buttons, on_submit, ..
            } => {
                on_submit(buttons.selected().to_bool());
            }
            QuestionModal::Text {
                text_box,
                on_submit,
                ..
            } => {
                on_submit(text_box.into_text());
            }
        }
    }
}

impl Component for QuestionModal {
    fn id(&self) -> ComponentId {
        match self {
            QuestionModal::Confirm { id, .. }
            | QuestionModal::Text { id, .. } => *id,
        }
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        match self {
            QuestionModal::Confirm { buttons, .. } => {
                vec![buttons.to_child()]
            }
            QuestionModal::Text { text_box, .. } => {
                vec![text_box.to_child()]
            }
        }
    }
}

impl Draw for QuestionModal {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        match self {
            QuestionModal::Confirm { buttons, .. } => {
                canvas.draw(buttons, (), metadata.area(), true);
            }
            QuestionModal::Text { text_box, .. } => canvas.draw(
                text_box,
                TextBoxProps::default(),
                metadata.area(),
                true,
            ),
        }
    }
}

/// Draw props for any collapsible sidebar
#[derive(Default)]
pub struct SidebarProps {
    pub format: SidebarFormat,
}

impl SidebarProps {
    /// Draw the sidebar in collapsed/header mode, where just the selected
    /// value is visit
    pub fn header() -> Self {
        Self {
            format: SidebarFormat::Header,
        }
    }

    /// Draw the sidebar in list mode, where the entire list is visible and
    /// interactive
    pub fn list() -> Self {
        Self {
            format: SidebarFormat::List,
        }
    }
}

/// Visual format of a sidebar list
#[derive(Debug, Default)]
pub enum SidebarFormat {
    /// List is collapsed and just visible as a header. Only the selected value
    /// is visible
    Header,
    /// List is open in the sidebar and the entire list is visible
    #[default]
    List,
}

/// Emitted event from any sidebar
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum SidebarEvent {
    /// Sidebar should be expanded
    Open,
    /// Sidebar should be reset to the default state
    Reset,
}
