use crate::{
    context::TuiContext,
    view::{
        Generate, UpdateContext,
        common::{
            component_select::{
                ComponentSelect, ComponentSelectProps, SelectStyles,
            },
            select::{Select, SelectListProps},
            text_box::{TextBox, TextBoxProps},
        },
        component::{
            Canvas, Child, Component, ComponentId, Draw, DrawMetadata, ToChild,
        },
        event::{Event, EventMatch},
    },
};
use itertools::Itertools;
use ratatui::{
    layout::{Constraint, Layout, Spacing},
    prelude::{Buffer, Rect},
    text::{Line, Span, Text},
    widgets::Widget,
};
use slumber_config::Action;
use slumber_core::render::{Prompt, ResponseChannel, SelectOption};
use slumber_template::Value;
use std::{borrow::Cow, cmp, mem};

/// A form displaying prompts for the recipe builder
///
/// The TUI implementation of [Prompter](slumber_core::render::Prompter) sends
/// prompts here via the message queue. Whenever this has at least one prompt,
/// it should be shown. When the form is submitted, all prompts are submitted
/// together, clearing the queue.
#[derive(Debug, Default)]
pub struct PromptForm {
    id: ComponentId,
    /// All inputs in the form. We use a select for this to manage the focus
    /// on one input at a time
    select: ComponentSelect<PromptInput>,
    /// Are we in edit mode? User has to explicitly switch to editing. This
    /// flag is retained when switching fields, so the user doesn't have to
    /// edit each field individually.
    editing: bool,
}

impl PromptForm {
    /// Prompt the user for input
    pub fn prompt(&mut self, prompt: Prompt) {
        // Selects are immutable, so we have to rebuild with the new prompt
        // appended
        let select = mem::take(&mut self.select).into_select();
        let selected_index = select.selected_index().unwrap_or(0);
        let mut items = select.into_items().collect_vec();
        items.push(PromptInput::new(prompt));
        self.select = Select::builder(items)
            // Carry over the previous selected index. It's possible for
            // additional prompts to come in during the edit, and we don't want
            // to reset select state in that case
            .preselect_index(selected_index)
            .build()
            .into();
    }

    /// Are there any prompts in the queue? When this is true, we show the form
    pub fn has_prompts(&self) -> bool {
        !self.select.is_empty()
    }
}

impl Component for PromptForm {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn update(
        &mut self,
        _context: &mut UpdateContext,
        event: Event,
    ) -> EventMatch {
        event.m().action(|action, propagate| match action {
            Action::PreviousPane => self.select.up(),
            Action::NextPane => self.select.down(),
            Action::Edit if !self.editing => self.editing = true,
            Action::Cancel => {
                if self.editing {
                    // If editing, just exit editing
                    self.editing = false;
                } else {
                    // Clear out all inputs. This will drop all the prompts,
                    // triggering an error in the request
                    self.select = ComponentSelect::default();
                }
            }
            Action::Submit => {
                if self.editing {
                    // If editing, just exit editing
                    self.editing = false;
                } else {
                    // Tell each input to submit its response to its channel
                    let select = mem::take(&mut self.select).into_select();
                    for input in select.into_items() {
                        input.submit();
                    }
                }
            }
            _ => propagate.set(),
        })
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        vec![self.select.to_child_mut()]
    }
}

impl Draw for PromptForm {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        let [form_area, help_area] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(1)])
                .areas(metadata.area());

        let input_engine = &TuiContext::get().input_engine;
        let styles = &TuiContext::get().styles;
        let help = format!(
            "Change Field {previous}/{next} | Submit {submit} | Cancel {cancel}",
            previous = input_engine.binding_display(Action::PreviousPane),
            next = input_engine.binding_display(Action::NextPane),
            submit = input_engine.binding_display(Action::Submit),
            cancel = input_engine.binding_display(Action::Cancel),
        );
        canvas
            .render_widget(Line::from(help).style(styles.text.hint), help_area);

        let props = PromptInputProps {
            editing: self.editing,
        };
        canvas.draw(
            &self.select,
            ComponentSelectProps {
                styles: SelectStyles::none(),
                spacing: Spacing::default(),
                item_props: Box::new(move |item, is_selected| {
                    // Let each item determine its own height
                    (props, item.height(is_selected && props.editing))
                }),
            },
            form_area,
            true,
        );
    }
}

/// A single input in a prompt form
#[derive(Debug)]
enum PromptInput {
    /// Prompt the user for text input
    Text {
        id: ComponentId,
        message: String,
        text_box: TextBox,
        channel: ResponseChannel<String>,
    },
    /// Prompt the user to select an item from a list
    Select {
        id: ComponentId,
        message: String,
        /// List of options to present to the user
        select: Select<SelectOption>,
        channel: ResponseChannel<Value>,
    },
}

impl PromptInput {
    fn new(prompt: Prompt) -> Self {
        match prompt {
            Prompt::Text {
                message,
                default,
                sensitive,
                channel,
            } => Self::Text {
                id: ComponentId::default(),
                message,

                text_box: TextBox::default()
                    .sensitive(sensitive)
                    .default_value(default.unwrap_or_default()),
                channel,
            },
            Prompt::Select {
                message,
                options,
                channel,
            } => Self::Select {
                id: ComponentId::default(),
                message,
                select: Select::builder(options).build(),
                channel,
            },
        }
    }

    /// Get the intended draw height of this input, including its header
    fn height(&self, editing: bool) -> u16 {
        let content_height = match self {
            // When a select is editable, we show the entire list
            PromptInput::Select { select, .. } if editing => {
                // Cap the height of the list to prevent taking up too much
                // space
                cmp::min(select.len() as u16, 5)
            }
            // 1 for title, 1 for input
            PromptInput::Text { .. } | PromptInput::Select { .. } => 1,
        };
        // +1 for the field header
        content_height + 1
    }

    /// Submit the current input/selection to the response channel
    fn submit(self) {
        match self {
            PromptInput::Text {
                text_box, channel, ..
            } => {
                channel.respond(text_box.into_text());
            }
            PromptInput::Select {
                select, channel, ..
            } => {
                // Empty select should be prevented by the render engine
                if let Some(option) = select.into_selected() {
                    channel.respond(option.value);
                }
            }
        }
    }
}

impl Component for PromptInput {
    fn id(&self) -> ComponentId {
        match self {
            PromptInput::Text { id, .. } | PromptInput::Select { id, .. } => {
                *id
            }
        }
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        match self {
            Self::Text { text_box, .. } => vec![text_box.to_child_mut()],
            Self::Select { select, .. } => vec![select.to_child_mut()],
        }
    }
}

impl Draw<PromptInputProps> for PromptInput {
    fn draw(
        &self,
        canvas: &mut Canvas,
        props: PromptInputProps,
        metadata: DrawMetadata,
    ) {
        let [title_area, content_area] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(1)])
                .areas(metadata.area());

        // Draw the title
        canvas.render_widget(
            InputTitle {
                input: self,
                editing: props.editing,
                has_focus: metadata.has_focus(),
            },
            title_area,
        );

        if metadata.has_focus() && props.editing {
            // If focused + editing, show the interactive content
            match self {
                PromptInput::Text { text_box, .. } => canvas.draw(
                    text_box,
                    TextBoxProps::default(),
                    content_area,
                    true,
                ),
                PromptInput::Select { select, .. } => canvas.draw(
                    select,
                    SelectListProps::pane(),
                    content_area,
                    true,
                ),
            }
        } else {
            // Content is just a string
            let value: Cow<'_, str> = match self {
                PromptInput::Text { text_box, .. } => text_box.display_text(),
                PromptInput::Select { select, .. } => {
                    select
                        .selected()
                        .map(|item| item.label.as_str())
                        // Empty list should be prevented by the render engine
                        .unwrap_or("<None>")
                        .into()
                }
            };
            canvas.render_widget(value.as_ref(), content_area);
        }
    }
}

/// Draw props for [PromptInput]
#[derive(Copy, Clone)]
struct PromptInputProps {
    editing: bool,
}

/// Widget to draw the title line of a form field
struct InputTitle<'a> {
    input: &'a PromptInput,
    editing: bool,
    has_focus: bool,
}

impl Widget for InputTitle<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let styles = &TuiContext::get().styles;

        let title_style = if self.has_focus {
            styles.form.title_highlight
        } else {
            styles.form.title
        };
        let mut title = Line::default();
        let field_name = match self.input {
            PromptInput::Text { message, .. }
            | PromptInput::Select { message, .. } => message.as_str(),
        };
        title.push_span(Span::from(field_name).style(title_style));

        // If focused, show a hint
        if self.has_focus {
            let input_engine = &TuiContext::get().input_engine;
            let hint = if self.editing {
                format!(
                    " Done {}",
                    input_engine.binding_display(Action::Submit)
                )
            } else {
                format!(" Edit {}", input_engine.binding_display(Action::Edit))
            };
            title.push_span(Span::from(hint).style(styles.text.hint));
        }

        title.render(area, buf);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_util::{TestHarness, TestTerminal, harness, terminal},
        view::test_util::TestComponent,
    };
    use ratatui::style::{Style, Styled};
    use rstest::rstest;
    use terminput::{KeyCode, KeyModifiers};
    use tokio::sync::oneshot::{self, Receiver, error::TryRecvError};

    /// Navigate between multiple fields and submit
    #[rstest]
    fn test_navigation(harness: TestHarness, terminal: TestTerminal) {
        let mut component =
            TestComponent::new(&harness, &terminal, PromptForm::default());
        let (username_prompt, mut username_rx) =
            text("Username", Some("user"), false);
        component.prompt(username_prompt);
        let (species_prompt, mut species_rx) = select(
            "Species",
            vec![
                ("holy shit what is that thing", 1.into()),
                ("it's a baby fuckin wheel!", 2.into()),
                ("look at that thing jay!", 3.into()),
            ],
        );
        component.prompt(species_prompt);
        let (password_prompt, mut password_rx) =
            text("Password", Some("hunter2"), true);
        component.prompt(password_prompt);

        component
            .int()
            .drain_draw() // Draw so children are visible
            .send_key(KeyCode::Char('e')) // Edit
            .send_text("123") // Modify username
            .send_key(KeyCode::Tab) // Switch to species - still editing
            .send_key(KeyCode::Down) // Select 2nd option
            .send_key_modifiers(KeyCode::Tab, KeyModifiers::SHIFT) // Go back
            .send_text("4") // Modify username again
            .send_key(KeyCode::Up) // We can navigate with arrow keys too
            .send_text("456") // Modify password
            .send_key(KeyCode::Enter) // Done editing
            .send_key(KeyCode::Enter) // Submit
            .assert_empty();

        assert_eq!(username_rx.try_recv().unwrap(), "user1234");
        assert_eq!(species_rx.try_recv().unwrap(), 2.into());
        assert_eq!(password_rx.try_recv().unwrap(), "hunter2456");
    }

    /// Cancelling should drop all the responders, triggering errors
    #[rstest]
    fn test_cancel(harness: TestHarness, terminal: TestTerminal) {
        let mut component =
            TestComponent::new(&harness, &terminal, PromptForm::default());
        let (prompt, mut rx) = text("Username", Some("user"), false);
        component.prompt(prompt);

        component
            .int()
            .drain_draw() // Draw so children are visible
            .send_key(KeyCode::Esc)
            .assert_empty();

        // Channel was closed
        assert_eq!(rx.try_recv(), Err(TryRecvError::Closed));
    }

    /// Text input field
    #[rstest]
    fn test_text(harness: TestHarness, #[with(8, 5)] terminal: TestTerminal) {
        let mut component =
            TestComponent::new(&harness, &terminal, PromptForm::default());
        let (username_prompt, mut username_rx) =
            text("Username", Some("user"), false);
        component.prompt(username_prompt);
        let (password_prompt, mut password_rx) =
            text("Password", Some("hunter"), true);
        component.prompt(password_prompt);

        component
            .int()
            .drain_draw() // Draw so children are visible
            .send_key(KeyCode::Char('e')) // Edit
            .send_text("12") // Modify username
            .send_key(KeyCode::Tab) // Switch to password
            .send_text("2") // Modify password
            .assert_empty();

        // Check terminal contents
        let styles = &TuiContext::get().styles;
        terminal.assert_buffer_lines([
            Line::styled("Username", styles.form.title),
            Line::styled("user12", Style::default()),
            Line::styled("Password", styles.form.title_highlight),
            // Sensitive fields get masked, even when not editing
            Line::from_iter([
                "•••••••".set_style(styles.text_box.text),
                " ".set_style(styles.text_box.cursor),
            ]),
            // Footer gets cut off
            Line::styled("Change F", styles.text.hint),
        ]);

        // Submit
        component
            .int()
            // Done editing, then submit
            .send_keys([KeyCode::Enter, KeyCode::Enter])
            .assert_empty();
        assert_eq!(username_rx.try_recv().unwrap(), "user12");
        assert_eq!(password_rx.try_recv().unwrap(), "hunter2");
    }

    /// Select input field
    #[rstest]
    fn test_select(harness: TestHarness, #[with(7, 5)] terminal: TestTerminal) {
        let mut component =
            TestComponent::new(&harness, &terminal, PromptForm::default());
        let (prompt, mut rx) = select(
            "Species",
            vec![
                ("holy shit what is that thing", 1.into()),
                ("it's a baby fuckin wheel!", 2.into()),
                ("look at that thing jay!", 3.into()),
            ],
        );
        component.prompt(prompt);

        component
            .int()
            .drain_draw() // Draw so children are visible
            .send_keys([KeyCode::Char('e'), KeyCode::Down])
            .assert_empty();

        // Check terminal contents
        let styles = &TuiContext::get().styles;
        terminal.assert_buffer_lines([
            Line::styled("Species", styles.form.title_highlight),
            Line::styled("holy sh", Style::default()),
            Line::styled("it's a ", styles.list.highlight),
            Line::styled("look at", Style::default()),
            // Footer gets cut off
            Line::styled("Change ", styles.text.hint),
        ]);

        // Submit
        component
            .int()
            // Done editing, then submit
            .send_keys([KeyCode::Enter, KeyCode::Enter])
            .assert_empty();
        assert_eq!(rx.try_recv().unwrap(), 2.into());
    }

    /// When a new field is added to the form, whatever field was previously
    /// selected should remain selected
    #[rstest]
    fn test_retain_selected_field(
        harness: TestHarness,
        terminal: TestTerminal,
    ) {
        let mut component =
            TestComponent::new(&harness, &terminal, PromptForm::default());
        component.prompt(text("Username", Some("user"), false).0);
        component.prompt(select("Select", vec![]).0);

        component
            .int()
            .drain_draw() // Draw so children are visible
            .send_key(KeyCode::Tab)
            .assert_empty();
        assert_eq!(component.select.selected_index(), Some(1));

        component.prompt(text("Password", Some("hunter2"), true).0);
        // Selection state is *not* lost
        assert_eq!(component.select.selected_index(), Some(1));
    }

    /// Create a text prompt
    fn text(
        message: &str,
        default: Option<&str>,
        sensitive: bool,
    ) -> (Prompt, Receiver<String>) {
        let (tx, rx) = oneshot::channel();
        let prompt = Prompt::Text {
            message: message.into(),
            default: default.map(String::from),
            sensitive,
            channel: tx.into(),
        };
        (prompt, rx)
    }

    /// Create a select prompt
    fn select(
        message: &str,
        options: Vec<(&str, Value)>,
    ) -> (Prompt, Receiver<Value>) {
        let (tx, rx) = oneshot::channel();
        let prompt = Prompt::Select {
            message: message.into(),
            options: options
                .into_iter()
                .map(|(label, value)| SelectOption {
                    label: label.into(),
                    value,
                })
                .collect(),
            channel: tx.into(),
        };
        (prompt, rx)
    }
}
