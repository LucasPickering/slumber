//! Miscellaneous components. They have specific purposes and therefore aren't
//! generic/utility, but don't fall into a clear category.

use crate::view::{
    common::{
        button::ButtonGroup,
        list::List,
        modal::{IntoModal, Modal},
        text_box::TextBox,
    },
    component::Component,
    draw::{Draw, DrawMetadata, Generate},
    event::{Child, Event, EventHandler, Update},
    state::{select::SelectState, Notification},
    Confirm, ModalPriority, ViewContext,
};
use derive_more::Display;
use ratatui::{
    prelude::Constraint,
    text::{Line, Text},
    widgets::{Paragraph, Wrap},
    Frame,
};
use slumber_core::template::{Prompt, PromptChannel, Select};
use std::{cell::Cell, fmt::Debug, rc::Rc};
use strum::{EnumCount, EnumIter};

#[derive(Debug)]
pub struct ErrorModal(anyhow::Error);

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

impl EventHandler for ErrorModal {}

impl Draw for ErrorModal {
    fn draw(&self, frame: &mut Frame, _: (), metadata: DrawMetadata) {
        frame.render_widget(
            Paragraph::new(self.0.generate()).wrap(Wrap::default()),
            metadata.area(),
        );
    }
}

impl IntoModal for anyhow::Error {
    type Target = ErrorModal;

    fn into_modal(self) -> Self::Target {
        ErrorModal(self)
    }
}

/// A modal with a single text box. The user will either enter some text and
/// submit it, or cancel.
#[derive(derive_more::Debug)]
pub struct TextBoxModal {
    /// Modal title, from the prompt message
    title: String,
    /// Little editor fucker
    text_box: Component<TextBox>,
    /// Flag set before closing to indicate if we should submit in our own
    /// `on_close`. This is set from the text box's `on_submit`.
    submit: Rc<Cell<bool>>,
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
        let submit = Rc::new(Cell::new(false));
        let submit_cell = Rc::clone(&submit);
        let text_box = text_box
            // Make sure cancel gets propagated to close the modal
            .on_cancel(|| ViewContext::push_event(Event::CloseModal))
            .on_submit(move || {
                // We have to defer submission to on_close, because we need the
                // owned value of `self.prompt`. We could have just put that in
                // a refcell, but this felt a bit cleaner because we know this
                // submitter will only be called once.
                submit_cell.set(true);
                ViewContext::push_event(Event::CloseModal);
            })
            .into();
        Self {
            title,
            text_box,
            submit,
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

    fn on_close(self: Box<Self>) {
        if self.submit.get() {
            // Return the user's value and close the prompt
            (self.on_submit)(self.text_box.into_data().into_text());
        }
    }
}

impl EventHandler for TextBoxModal {
    fn children(&mut self) -> Vec<Component<Child<'_>>> {
        vec![self.text_box.to_child_mut()]
    }
}

impl Draw for TextBoxModal {
    fn draw(&self, frame: &mut Frame, _: (), metadata: DrawMetadata) {
        self.text_box.draw(frame, (), metadata.area(), true);
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
    /// Modal title, from the select message
    title: String,
    /// List of options to present to the user
    options: Component<SelectState<String>>,
    /// Flag set before closing to indicate if we should submit in our own
    /// `on_close`. This is set from the text box's `on_submit`.
    submit: Rc<Cell<bool>>,
    #[debug(skip)]
    on_submit: Box<dyn 'static + FnOnce(String)>,
}

impl SelectListModal {
    /// Create a modal that contains a list of options.
    pub fn new(
        title: String,
        options: Vec<String>,
        on_submit: impl 'static + FnOnce(String),
    ) -> Self {
        // The underlying SelectState may close the modal
        // either because the user selected a value or left the modal
        // We use `submit` to inform our modal that the user selected a value
        let submit = Rc::new(Cell::new(false));
        let submit_cell = Rc::clone(&submit);
        Self {
            title,
            options: SelectState::builder(options)
                .on_submit(move |_selection| {
                    submit_cell.set(true);
                    ViewContext::push_event(Event::CloseModal);
                })
                .build()
                .into(),
            submit,
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
        let options = self.options.data();
        let longest_option =
            options.items().map(|s| s.len()).max().unwrap_or(10);
        // find our widest string to appropriately set width
        let width = std::cmp::max(self.title.len(), longest_option);
        (
            Constraint::Length(width as u16),
            Constraint::Length(options.len().min(20) as u16),
        )
    }

    fn on_close(self: Box<Self>) {
        // The modal is closed, but only submit the value if it was closed
        // because the user selected a value (submitted).
        if self.submit.get() {
            // Return the user's value and close the prompt
            (self.on_submit)(self.options.data().selected().unwrap().clone());
        }
    }
}

impl EventHandler for SelectListModal {
    fn children(&mut self) -> Vec<Component<Child<'_>>> {
        vec![self.options.to_child_mut()]
    }
}

impl Draw for SelectListModal {
    fn draw(&self, frame: &mut Frame, _: (), metadata: DrawMetadata) {
        // Empty state
        let options = self.options.data();
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
            self.channel.respond(response)
        })
    }
}

/// Inner state for the prompt modal
#[derive(Debug)]
pub struct ConfirmModal {
    /// Modal title, from the prompt message
    title: String,
    /// Channel used to submit yes/no. This is an option so we can take the
    /// value when a submission is given, and then close the modal. It should
    /// only ever be taken once.
    channel: Option<PromptChannel<bool>>,
    buttons: Component<ButtonGroup<ConfirmButton>>,
}

/// Buttons in the confirmation modal
#[derive(
    Copy, Clone, Debug, Default, Display, EnumCount, EnumIter, PartialEq,
)]
enum ConfirmButton {
    No,
    #[default]
    Yes,
}

impl ConfirmModal {
    pub fn new(confirm: Confirm) -> Self {
        Self {
            title: confirm.message,
            channel: Some(confirm.channel),
            buttons: Default::default(),
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
            Constraint::Length((self.title.len() + 4) as u16),
            Constraint::Length(1),
        )
    }
}

impl EventHandler for ConfirmModal {
    fn update(&mut self, event: Event) -> Update {
        // When user selects a button, send the response and close
        let Some(button) = event.local::<ConfirmButton>() else {
            return Update::Propagate(event);
        };
        // Channel *should* always be available here, because after handling
        // this event for the first time we close the modal. Hypothetically we
        // could get two submissions in rapid succession though, so ignore
        // subsequent ones.
        if let Some(channel) = self.channel.take() {
            channel.respond(*button == ConfirmButton::Yes);
        }

        ViewContext::push_event(Event::CloseModal);
        Update::Consumed
    }

    fn children(&mut self) -> Vec<Component<Child<'_>>> {
        vec![self.buttons.to_child_mut()]
    }
}

impl Draw for ConfirmModal {
    fn draw(&self, frame: &mut Frame, _: (), metadata: DrawMetadata) {
        self.buttons.draw(frame, (), metadata.area(), true);
    }
}

impl IntoModal for Confirm {
    type Target = ConfirmModal;

    fn into_modal(self) -> Self::Target {
        ConfirmModal::new(self)
    }
}

/// Show most recent notification with timestamp
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
    fn draw(&self, frame: &mut Frame, _: (), metadata: DrawMetadata) {
        frame.render_widget(
            Paragraph::new(self.notification.generate()),
            metadata.area(),
        );
    }
}
