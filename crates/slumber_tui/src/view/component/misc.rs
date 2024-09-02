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
use slumber_core::template::{Prompt, Select};
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
        let text_box = text_box
            // Make sure cancel gets propagated to close the modal
            .on_cancel(|| {
                ViewContext::push_event(Event::CloseModal { submitted: false })
            })
            .on_submit(move || {
                // We have to defer submission to on_close, because we need the
                // owned value of `self.prompt`
                ViewContext::push_event(Event::CloseModal { submitted: true });
            })
            .into();
        Self {
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

    fn on_close(self: Box<Self>, submitted: bool) {
        if submitted {
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
        Self {
            title,
            options: SelectState::builder(options)
                .on_submit(move |_| {
                    ViewContext::push_event(Event::CloseModal {
                        submitted: true,
                    });
                })
                .build()
                .into(),
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

    fn on_close(self: Box<Self>, submitted: bool) {
        // The modal is closed, but only submit the value if it was closed
        // because the user selected a value (submitted).
        if submitted {
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
#[derive(derive_more::Debug)]
pub struct ConfirmModal {
    /// Modal title, from the prompt message
    title: String,
    buttons: Component<ButtonGroup<ConfirmButton>>,
    /// Store which answer was selected during submission. Answering no is
    /// semantically different from not answering, so we can't just check the
    /// `submitted` flag in `on_close`
    answer: bool,
    #[debug(skip)]
    on_submit: Box<dyn 'static + FnOnce(bool)>,
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
    pub fn new(title: String, on_submit: impl 'static + FnOnce(bool)) -> Self {
        Self {
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
            Constraint::Length((self.title.len() + 4) as u16),
            Constraint::Length(1),
        )
    }

    fn on_close(self: Box<Self>, submitted: bool) {
        if submitted {
            (self.on_submit)(self.answer);
        }
    }
}

impl EventHandler for ConfirmModal {
    fn update(&mut self, event: Event) -> Update {
        // When user selects a button, send the response and close
        let Some(button) = event.local::<ConfirmButton>() else {
            return Update::Propagate(event);
        };

        self.answer = *button == ConfirmButton::Yes;
        ViewContext::push_event(Event::CloseModal { submitted: true });
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
        ConfirmModal::new(self.message, |response| {
            self.channel.respond(response)
        })
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
