//! Miscellaneous components. They have specific purposes and therefore aren't
//! generic/utility, but don't fall into a clear category.

use crate::{
    util::ResultReported,
    view::{
        Confirm, Generate, ModalPriority, ViewContext,
        common::{
            button::ButtonGroup,
            list::List,
            modal::{IntoModal, Modal},
            text_box::{TextBox, TextBoxEvent, TextBoxProps},
        },
        component::{
            Component, ComponentExt, ComponentId, Draw, DrawMetadata,
            internal::{Child, ToChild},
        },
        context::UpdateContext,
        event::{Event, OptionEvent, ToEmitter},
        state::select::{SelectState, SelectStateEvent, SelectStateEventType},
    },
};
use derive_more::Display;
use ratatui::{
    Frame,
    prelude::Constraint,
    text::{Line, Text},
};
use slumber_core::{
    collection::{ProfileId, RecipeId},
    database::ProfileFilter,
    http::RequestId,
    render::{Prompt, Select, SelectOption},
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

impl Modal for ErrorModal {
    fn priority(&self) -> ModalPriority {
        ModalPriority::High // beep beep coming through
    }

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
    fn draw_impl(&self, frame: &mut Frame, (): (), metadata: DrawMetadata) {
        frame.render_widget(self.error.generate(), metadata.area());
    }
}

impl IntoModal for anyhow::Error {
    type Target = ErrorModal;

    fn into_modal(self) -> Self::Target {
        ErrorModal {
            id: ComponentId::default(),
            error: self,
        }
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
            text_box: text_box.subscribe([TextBoxEvent::Submit]),
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

    fn on_close(self: Box<Self>, submitted: bool) {
        if submitted {
            // Return the user's value and close the prompt
            (self.on_submit)(self.text_box.into_text());
        }
    }
}

impl Component for TextBoxModal {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn update(&mut self, _: &mut UpdateContext, event: Event) -> Option<Event> {
        event
            .opt()
            .emitted(self.text_box.to_emitter(), |event| match event {
                TextBoxEvent::Submit => {
                    // We have to defer submission to on_close, because we need
                    // the owned value of `self.on_submit`
                    self.close(true);
                }
                TextBoxEvent::Focus
                | TextBoxEvent::Change
                | TextBoxEvent::Cancel => {}
            })
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        vec![self.text_box.to_child_mut()]
    }
}

impl Draw for TextBoxModal {
    fn draw_impl(&self, frame: &mut Frame, (): (), metadata: DrawMetadata) {
        self.text_box.draw(
            frame,
            TextBoxProps::default(),
            metadata.area(),
            true,
        );
    }
}

/// Present a prompt as a modal to the user
impl IntoModal for Prompt {
    type Target = TextBoxModal;

    fn into_modal(self) -> Self::Target {
        TextBoxModal::new(
            self.message,
            TextBox::default()
                .sensitive(self.sensitive)
                .default_value(self.default.unwrap_or_default()),
            |response| self.channel.respond(response),
        )
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
    options: SelectState<SelectOption>,
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
            options: SelectState::builder(options)
                .subscribe([SelectStateEventType::Submit])
                .build(),
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

    fn on_close(self: Box<Self>, submitted: bool) {
        // The modal is closed, but only submit the value if it was closed
        // because the user selected a value (submitted).
        if submitted {
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
}

impl Component for SelectListModal {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn update(&mut self, _: &mut UpdateContext, event: Event) -> Option<Event> {
        event.opt().emitted(self.options.to_emitter(), |event| {
            if let SelectStateEvent::Submit(_) = event {
                self.close(true);
            }
        })
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        vec![self.options.to_child_mut()]
    }
}

impl Draw for SelectListModal {
    fn draw_impl(&self, frame: &mut Frame, (): (), metadata: DrawMetadata) {
        // Empty state
        let options = &self.options;
        if options.is_empty() {
            frame.render_widget(
                Text::from(vec!["No options defined!".into()]),
                metadata.area(),
            );
            return;
        }

        self.options
            .draw(frame, List::from(options), metadata.area(), true);
    }
}

/// Present a select list as a modal to the user
impl IntoModal for Select {
    type Target = SelectListModal;

    fn into_modal(self) -> Self::Target {
        SelectListModal::new(self.message, self.options, |response| {
            self.channel.respond(response);
        })
    }
}

/// Render a select option via its label
impl Generate for &SelectOption {
    type Output<'this>
        = Text<'this>
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

/// Inner state for the prompt modal
#[derive(derive_more::Debug)]
pub struct ConfirmModal {
    id: ComponentId,
    /// Modal title, from the prompt message
    title: String,
    buttons: ButtonGroup<ConfirmButton>,
    /// Store which answer was selected during submission. Answering no is
    /// semantically different from not answering, so we can't just check the
    /// `submitted` flag in `on_close`
    answer: bool,
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
            answer: false,
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

    fn on_close(self: Box<Self>, submitted: bool) {
        if submitted {
            (self.on_submit)(self.answer);
        }
    }
}

impl Component for ConfirmModal {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn update(&mut self, _: &mut UpdateContext, event: Event) -> Option<Event> {
        event.opt().emitted(self.buttons.to_emitter(), |button| {
            // When user selects a button, send the response and close
            self.answer = button == ConfirmButton::Yes;
            // If the user answers, then they submitted a response, even if the
            // answer was no
            self.close(true);
        })
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        vec![self.buttons.to_child_mut()]
    }
}

impl Draw for ConfirmModal {
    fn draw_impl(&self, frame: &mut Frame, (): (), metadata: DrawMetadata) {
        self.buttons.draw(frame, (), metadata.area(), true);
    }
}

impl IntoModal for Confirm {
    type Target = ConfirmModal;

    fn into_modal(self) -> Self::Target {
        ConfirmModal::new(self.message, |response| {
            self.channel.respond(response);
        })
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
}

impl Component for DeleteRequestModal {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn update(
        &mut self,
        context: &mut UpdateContext,
        event: Event,
    ) -> Option<Event> {
        event.opt().emitted(self.buttons.to_emitter(), |button| {
            // Do the delete here because we have access to the request store
            if button == ConfirmButton::Yes {
                context
                    .request_store
                    .delete_request(self.request_id)
                    .reported(&ViewContext::messages_tx());
                ViewContext::push_event(Event::HttpSelectRequest(None));
            }
            self.close(true);
        })
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        vec![self.buttons.to_child_mut()]
    }
}

impl Draw for DeleteRequestModal {
    fn draw_impl(&self, frame: &mut Frame, (): (), metadata: DrawMetadata) {
        self.buttons.draw(frame, (), metadata.area(), true);
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

impl Component for DeleteRecipeRequestsModal {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn update(
        &mut self,
        context: &mut UpdateContext,
        event: Event,
    ) -> Option<Event> {
        event.opt().emitted(self.buttons.to_emitter(), |button| {
            // Do the delete here because we have access to the request store
            let profile_filter = match button {
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
            self.close(true);
        })
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        vec![self.buttons.to_child_mut()]
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
}

impl Draw for DeleteRecipeRequestsModal {
    fn draw_impl(&self, frame: &mut Frame, (): (), metadata: DrawMetadata) {
        self.buttons.draw(frame, (), metadata.area(), true);
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
