//! Miscellaneous components. They have specific purposes and therefore aren't
//! generic/utility, but don't fall into a clear category.

use crate::{
    util::ResultReported,
    view::{
        Generate, ViewContext,
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
        state::select::{Select, SelectListProps},
    },
};
use derive_more::Display;
use ratatui::{
    prelude::Constraint,
    text::{Line, Span, Text},
};
use slumber_core::{
    collection::{ProfileId, RecipeId},
    database::ProfileFilter,
    http::RequestId,
    render::SelectOption,
};
use slumber_template::Value;
use std::fmt::Debug;
use strum::{EnumCount, EnumIter};

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

/// A modal with a single text box. The user will either enter some text and
/// submit it, or cancel.
#[derive(derive_more::Debug)]
pub struct TextBoxModal {
    id: ComponentId,
    /// Modal title, from the prompt message
    title: String,
    /// Little editor fucker
    text_box: TextBox,
    /// Callback when the user hits Enter
    #[debug(skip)]
    on_submit: Box<dyn 'static + FnOnce(String)>,
}

impl TextBoxModal {
    /// Create a modal that contains a single text box. You can customize the
    /// text box as you want, but **the `on_cancel` and `on_submit`** callbacks
    /// will be overridden**. Pass a separate `on_submit` instead (`on_cancel`
    /// not supported for this modal).
    pub fn new(
        title: String,
        text_box: TextBox,
        on_submit: impl 'static + FnOnce(String),
    ) -> Self {
        Self {
            id: ComponentId::default(),
            title,
            text_box,
            on_submit: Box::new(on_submit),
        }
    }
}

impl Modal for TextBoxModal {
    fn title(&self) -> Line<'_> {
        self.title.as_str().into()
    }

    fn dimensions(&self) -> (Constraint, Constraint) {
        (Constraint::Percentage(60), Constraint::Length(1))
    }

    fn on_submit(self, _: &mut UpdateContext) {
        // on_submit is called automatically because we *don't* subscribe to the
        // text box submit event. That means submission gets forwarded to the
        // parent modal handler
        (self.on_submit)(self.text_box.into_text());
    }
}

impl Component for TextBoxModal {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        vec![self.text_box.to_child_mut()]
    }
}

impl Draw for TextBoxModal {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        canvas.draw(
            &self.text_box,
            TextBoxProps::default(),
            metadata.area(),
            true,
        );
    }
}

/// A modal that presents a list of simple string options to the user.
/// The user will select one of the options and submit it, or cancel.
#[derive(derive_more::Debug)]
pub struct SelectListModal {
    id: ComponentId,
    /// Modal title, from the select message
    title: String,
    /// List of options to present to the user
    options: Select<SelectOption>,
    #[debug(skip)]
    on_submit: Box<dyn 'static + FnOnce(Value)>,
}

impl SelectListModal {
    /// Create a modal that contains a list of options.
    pub fn new(
        title: String,
        options: Vec<SelectOption>,
        on_submit: impl 'static + FnOnce(Value),
    ) -> Self {
        Self {
            id: ComponentId::default(),
            title,
            options: Select::builder(options).build(),
            on_submit: Box::new(on_submit),
        }
    }
}

impl Modal for SelectListModal {
    fn title(&self) -> Line<'_> {
        self.title.as_str().into()
    }

    fn dimensions(&self) -> (Constraint, Constraint) {
        // Do some simple math to size the select modal correctly to our
        // underlying data
        let longest_option = self
            .options
            .items()
            .map(|option| option.label.len())
            .max()
            .unwrap_or(10);
        // find our widest string to appropriately set width
        let width = std::cmp::max(self.title.len(), longest_option);
        (
            Constraint::Length(width as u16),
            Constraint::Length(self.options.len().min(20) as u16),
        )
    }

    fn on_submit(self, _: &mut UpdateContext) {
        // Return the user's value and close the prompt. Value can be empty
        // if the select list is empty
        let selected = self
            .options
            .into_selected()
            .map(|option| option.value)
            .unwrap_or_default();
        (self.on_submit)(selected);
    }
}

impl Component for SelectListModal {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        vec![self.options.to_child_mut()]
    }
}

impl Draw for SelectListModal {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        // Empty state
        if self.options.is_empty() {
            canvas.render_widget(
                Text::from(vec!["No options defined!".into()]),
                metadata.area(),
            );
        } else {
            canvas.draw(
                &self.options,
                SelectListProps::modal(),
                metadata.area(),
                true,
            );
        }
    }
}

/// Render a select option via its label
impl Generate for &SelectOption {
    type Output<'this>
        = Span<'this>
    where
        Self: 'this;

    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this,
    {
        self.label.as_str().into()
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

/// Inner state for the prompt modal
#[derive(derive_more::Debug)]
pub struct ConfirmModal {
    id: ComponentId,
    /// Modal title, from the prompt message
    title: String,
    buttons: ButtonGroup<ConfirmButton>,
    /// Callback when the user responses
    #[debug(skip)]
    on_submit: Box<dyn 'static + FnOnce(bool)>,
}

impl ConfirmModal {
    const MIN_WIDTH: u16 = 24;

    pub fn new(title: String, on_submit: impl 'static + FnOnce(bool)) -> Self {
        Self {
            id: ComponentId::default(),
            title,
            buttons: Default::default(),
            on_submit: Box::new(on_submit),
        }
    }
}

impl Modal for ConfirmModal {
    fn title(&self) -> Line<'_> {
        self.title.as_str().into()
    }

    fn dimensions(&self) -> (Constraint, Constraint) {
        (
            // Add some arbitrary padding
            Constraint::Length(
                Self::MIN_WIDTH.max((self.title.len() + 4) as u16),
            ),
            Constraint::Length(1),
        )
    }

    fn on_submit(self, _context: &mut UpdateContext) {
        // When user selects a button, send the response before closing
        let answer = self.buttons.selected().to_bool();
        (self.on_submit)(answer);
    }
}

impl Component for ConfirmModal {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        vec![self.buttons.to_child_mut()]
    }
}

impl Draw for ConfirmModal {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        canvas.draw(&self.buttons, (), metadata.area(), true);
    }
}

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
        (
            Constraint::Length(ConfirmModal::MIN_WIDTH),
            Constraint::Length(1),
        )
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
