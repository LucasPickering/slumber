//! Miscellaneous components. They have specific purposes and therefore aren't
//! generic/utility, but don't fall into a clear category.

use crate::{
    util::ResultReported,
    view::{
        Generate, Question, ViewContext,
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
        event::Event,
    },
};
use derive_more::Display;
use ratatui::{prelude::Constraint, text::Line};
use slumber_core::{
    collection::{ProfileId, RecipeId},
    database::ProfileFilter,
    http::RequestId,
};
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
        on_submit: QuestionCallback<bool>,
    },
    /// Free-form text response
    Text {
        id: ComponentId,
        message: String,
        text_box: TextBox,
        /// Callback when the user replies
        #[debug(skip)]
        on_submit: QuestionCallback<String>,
    },
}

impl QuestionModal {
    /// Open a modal with a yes/no question
    pub fn confirm(
        message: String,
        on_submit: impl 'static + FnOnce(&mut UpdateContext, bool),
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
        on_submit: impl 'static + FnOnce(&mut UpdateContext, String),
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
                Self::confirm(message, move |_, reply| channel.reply(reply))
            }
            Question::Text {
                message,
                default,
                channel,
            } => Self::text(message, default, move |_, reply| {
                channel.reply(reply);
            }),
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

    fn on_submit(self, context: &mut UpdateContext) {
        match self {
            QuestionModal::Confirm {
                buttons, on_submit, ..
            } => {
                on_submit(context, buttons.selected().to_bool());
            }
            QuestionModal::Text {
                text_box,
                on_submit,
                ..
            } => {
                on_submit(context, text_box.into_text());
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
                vec![buttons.to_child_mut()]
            }
            QuestionModal::Text { text_box, .. } => {
                vec![text_box.to_child_mut()]
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
            QuestionModal::Text { text_box, .. } => {
                canvas.draw(
                    text_box,
                    TextBoxProps::default(),
                    metadata.area(),
                    true,
                );
            }
        }
    }
}

type QuestionCallback<T> = Box<dyn 'static + FnOnce(&mut UpdateContext, T)>;

/// Confirmation modal to delete a single request
#[derive(Debug)]
pub struct DeleteRequestModal {
    id: ComponentId,
    request_id: RequestId,
    buttons: ButtonGroup<ConfirmButton>,
}

impl DeleteRequestModal {
    pub fn new(request_id: RequestId) -> Self {
        Self {
            id: ComponentId::default(),
            request_id,
            buttons: Default::default(),
        }
    }
}

impl Modal for DeleteRequestModal {
    fn title(&self) -> Line<'_> {
        "Delete Request?".into()
    }

    fn dimensions(&self) -> (Constraint, Constraint) {
        (Constraint::Length(24), Constraint::Length(1))
    }

    fn on_submit(self, context: &mut UpdateContext) {
        if self.buttons.selected().to_bool() {
            context
                .request_store
                .delete_request(self.request_id)
                .reported(&ViewContext::messages_tx());
            ViewContext::push_event(Event::HttpSelectRequest(None));
        }
    }
}

impl Component for DeleteRequestModal {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        vec![self.buttons.to_child_mut()]
    }
}

impl Draw for DeleteRequestModal {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        canvas.draw(&self.buttons, (), metadata.area(), true);
    }
}

/// Confirmation modal to delete all requests for a recipe
#[derive(Debug)]
pub struct DeleteRecipeRequestsModal {
    id: ComponentId,
    /// Currently selected profile. May be used for the profile filter,
    /// depending on what the user selects
    profile_id: Option<ProfileId>,
    recipe_id: RecipeId,
    buttons: ButtonGroup<DeleteRecipeRequestsButton>,
}

impl DeleteRecipeRequestsModal {
    pub fn new(profile_id: Option<ProfileId>, recipe_id: RecipeId) -> Self {
        Self {
            id: ComponentId::default(),
            profile_id,
            recipe_id,
            buttons: Default::default(),
        }
    }
}

impl Modal for DeleteRecipeRequestsModal {
    fn title(&self) -> Line<'_> {
        format!("Delete Requests for {}?", self.recipe_id).into()
    }

    fn dimensions(&self) -> (Constraint, Constraint) {
        const MIN_WIDTH: u16 = 44; // Enough room for the buttons
        (
            Constraint::Length(
                MIN_WIDTH.max((self.title().width() + 4) as u16),
            ),
            Constraint::Length(1),
        )
    }

    fn on_submit(self, context: &mut UpdateContext) {
        // Do the delete here because we have access to the request store
        let profile_filter = match self.buttons.selected() {
            DeleteRecipeRequestsButton::No => None,
            DeleteRecipeRequestsButton::Profile => {
                Some(self.profile_id.as_ref().into())
            }
            DeleteRecipeRequestsButton::All => Some(ProfileFilter::All),
        };
        if let Some(profile_filter) = profile_filter {
            context
                .request_store
                .delete_recipe_requests(profile_filter, &self.recipe_id)
                .reported(&ViewContext::messages_tx());
            ViewContext::push_event(Event::HttpSelectRequest(None));
        }
    }
}

impl Component for DeleteRecipeRequestsModal {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        vec![self.buttons.to_child_mut()]
    }
}

impl Draw for DeleteRecipeRequestsModal {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        canvas.draw(&self.buttons, (), metadata.area(), true);
    }
}

/// Buttons for [DeleteRecipeRequestsModal]
#[derive(
    Copy, Clone, Debug, Default, Display, EnumCount, EnumIter, PartialEq,
)]
enum DeleteRecipeRequestsButton {
    No,
    /// Delete requests only for the current profile
    #[default]
    #[display("For this profile")]
    Profile,
    /// Delete all requests for all profiles
    #[display("For all profiles")]
    All,
}
