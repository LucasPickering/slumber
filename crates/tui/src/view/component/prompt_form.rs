use crate::view::{
    Generate, UpdateContext, ViewContext,
    common::{
        component_select::{
            ComponentSelect, ComponentSelectProps, SelectStyles,
        },
        modal::Modal,
        select::{Select, SelectListProps},
        text_box::{TextBox, TextBoxProps},
    },
    component::{
        Canvas, Child, Component, ComponentId, Draw, DrawMetadata, ToChild,
    },
    event::{Event, EventMatch},
};
use itertools::Itertools;
use ratatui::{
    layout::{Constraint, Layout, Spacing},
    prelude::{Buffer, Rect},
    text::{Line, Span, Text},
    widgets::Widget,
};
use slumber_config::Action;
use slumber_core::{
    collection::{Recipe, RecipeId},
    http::RequestId,
    render::{Prompt, ReplyChannel, SelectOption},
};
use slumber_template::Value;
use std::{borrow::Cow, cmp, mem};

/// A form displaying prompts for the recipe builder
///
/// The TUI implementation of [Prompter](slumber_core::render::Prompter) sends
/// prompts here via the message queue. Whenever this has at least one prompt,
/// it should be shown. When the form is submitted, all prompts are submitted
/// together, clearing the queue.
#[derive(Debug)]
pub struct PromptForm {
    id: ComponentId,
    /// Recipe being built; used for the title
    recipe_name: String,
    /// Request being built
    request_id: RequestId,
    /// All inputs in the form. We use a select for this to manage the focus
    /// on one input at a time
    select: ComponentSelect<PromptInput>,
    /// Are we in edit mode? User has to explicitly switch to editing. This
    /// flag is retained when switching fields, so the user doesn't have to
    /// edit each field individually.
    editing: bool,
}

impl PromptForm {
    /// Create a new prompt form with one input for each prompt. A form should
    /// correspond to a single request.
    pub fn new(recipe_id: &RecipeId, request_id: RequestId) -> Self {
        let recipe_name = ViewContext::collection()
            .recipes
            .get_recipe(recipe_id)
            .map(Recipe::name)
            .unwrap_or("unknown")
            .to_owned();
        Self {
            id: ComponentId::new(),
            recipe_name,
            request_id,
            select: ComponentSelect::default(),
            editing: true,
        }
    }

    /// Add a new prompt to the bottom of the form
    pub fn add_prompt(&mut self, prompt: Prompt) {
        let input = PromptInput::new(prompt);
        // Select is immutable so we need to rebuild it. We can't clone the
        // prompts, so we have to replace it with an empty Select while
        // rebuilding.
        let select = mem::take(&mut self.select);
        let mut items = select.into_select().into_items().collect_vec();
        items.push(input);
        self.select = Select::builder(items).build().into();
    }

    pub fn request_id(&self) -> RequestId {
        self.request_id
    }

    /// Submit all prompts
    fn submit(self) {
        // Submit each prompt on its own channel
        for input in self.select.into_select().into_items() {
            input.submit();
        }
    }
}

impl Modal for PromptForm {
    fn title(&self) -> Line<'_> {
        self.recipe_name.as_str().into()
    }

    fn dimensions(&self) -> (Constraint, Constraint) {
        (Constraint::Percentage(60), Constraint::Percentage(60))
    }

    fn on_submit(self, _context: &mut UpdateContext) {
        self.submit();
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
            // If not editing, we'll propagate this to close the modal
            Action::Cancel if self.editing => self.editing = false,
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

        let styles = ViewContext::styles();
        let help = format!(
            "Change Field {previous}/{next} | Submit {submit} | Cancel {cancel}",
            previous = ViewContext::binding_display(Action::PreviousPane),
            next = ViewContext::binding_display(Action::NextPane),
            submit = ViewContext::binding_display(Action::Submit),
            cancel = ViewContext::binding_display(Action::Cancel),
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
        channel: ReplyChannel<String>,
    },
    /// Prompt the user to select an item from a list
    Select {
        id: ComponentId,
        message: String,
        /// List of options to present to the user
        select: Select<SelectOption>,
        channel: ReplyChannel<Value>,
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
                message: message.clone(),
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
                message: message.clone(),
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

    /// Submit the current selection/value over the reply channel
    fn submit(self) {
        match self {
            PromptInput::Text {
                text_box, channel, ..
            } => channel.reply(text_box.into_text()),
            PromptInput::Select {
                select, channel, ..
            } => {
                // Non-empty select is enforced by the select() function
                let option =
                    select.into_selected().expect("Select cannot be empty");
                channel.reply(option.value);
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
            canvas.render_widget(
                Line::styled(
                    value.as_ref(),
                    ViewContext::styles().form.content,
                ),
                content_area,
            );
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
        let styles = ViewContext::styles();

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
            let hint = if self.editing {
                format!(
                    " Exit {}",
                    ViewContext::binding_display(Action::Cancel)
                )
            } else {
                format!(" Edit {}", ViewContext::binding_display(Action::Edit))
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
    use crate::view::{
        common::modal::ModalQueue,
        test_util::{TestComponent, TestHarness, harness},
    };
    use ratatui::style::{Style, Styled};
    use rstest::rstest;
    use slumber_template::Value;
    use slumber_util::Factory;
    use terminput::{KeyCode, KeyModifiers};
    use tokio::sync::oneshot::{self, Receiver};

    /// Navigate between multiple fields and submission. Submission is handled
    /// by the parent ModalQueue, so we have to use that here too.
    #[rstest]
    fn test_navigation(mut harness: TestHarness) {
        let mut component =
            TestComponent::new(&mut harness, ModalQueue::default());
        // Build the form, then open its modal
        let mut form =
            PromptForm::new(&RecipeId::factory(()), RequestId::new());
        let mut username_rx = text(&mut form, "Username", Some("user"), false);
        let mut species_rx = select(
            &mut form,
            "Species",
            vec![
                ("holy shit what is that thing", 1.into()),
                ("it's a baby fuckin wheel!", 2.into()),
                ("look at that thing jay!", 3.into()),
            ],
        );
        let mut password_rx =
            text(&mut form, "Password", Some("hunter2"), true);
        component.open(form);

        component
            .int(&mut harness)
            .drain_draw() // Draw so children are visible
            .send_text("123") // Modify username
            .inspect(|modal_queue| {
                let form = modal_queue.first().unwrap();
                assert!(form.editing);
                assert_eq!(form.select.selected_index(), Some(0));
            })
            .send_key(KeyCode::Tab) // Switch to species - still editing
            .send_key(KeyCode::Down) // Select 2nd option
            // Exit edit mode, nav w/ arrow keys, then re-enter edit
            .send_keys([KeyCode::Esc, KeyCode::Up, KeyCode::Char('e')])
            .send_text("4") // Modify username again
            .send_key_modifiers(KeyModifiers::SHIFT, KeyCode::Up) // Wrap to pw
            .send_text("456") // Modify password
            .send_key(KeyCode::Enter) // Submit
            .assert()
            .empty();

        assert_eq!(username_rx.try_recv().unwrap(), "user1234".to_owned());
        assert_eq!(species_rx.try_recv().unwrap(), 2.into());
        assert_eq!(password_rx.try_recv().unwrap(), "hunter2456".to_owned());
    }

    /// Text input field
    #[rstest]
    fn test_text(#[with(8, 5)] mut harness: TestHarness) {
        let mut component = TestComponent::new(
            &mut harness,
            PromptForm::new(&RecipeId::factory(()), RequestId::new()),
        );
        let mut username_rx =
            text(&mut component, "Username", Some("user"), false);
        let mut password_rx =
            text(&mut component, "Password", Some("hunter"), true);

        component
            .int(&mut harness)
            .drain_draw() // Draw so children are visible
            .send_text("12") // Modify username
            .send_key(KeyCode::Tab) // Switch to password
            .send_text("2") // Modify password
            .assert()
            .empty();

        // Check terminal contents
        let styles = ViewContext::styles();
        harness.assert_buffer_lines([
            Line::styled("Username", styles.form.title),
            Line::styled("user12  ", styles.form.content),
            Line::styled("Password", styles.form.title_highlight),
            // Sensitive fields get masked
            Line::from_iter(["••••••• ".set_style(styles.text_box.text)]),
            // Footer gets cut off
            Line::styled("Change F", styles.text.hint),
        ]);
        harness.assert_cursor_position((7, 3));

        // Submit; submission event is handled by the ModalQueue. It's easier
        // just to call it manually since we test proper submission elsewhere
        component.into_inner().submit();
        assert_eq!(username_rx.try_recv().unwrap(), "user12".to_owned());
        assert_eq!(password_rx.try_recv().unwrap(), "hunter2".to_owned());
    }

    /// Select input field
    #[rstest]
    fn test_select(#[with(7, 5)] mut harness: TestHarness) {
        let mut component = TestComponent::new(
            &mut harness,
            PromptForm::new(&RecipeId::factory(()), RequestId::new()),
        );
        let mut species_rx = select(
            &mut component,
            "Species",
            vec![
                ("holy shit what is that thing", 1.into()),
                ("it's a baby fuckin wheel!", 2.into()),
                ("look at that thing jay!", 3.into()),
            ],
        );

        component
            .int(&mut harness)
            .drain_draw() // Draw so children are visible
            .send_key(KeyCode::Down)
            .assert()
            .empty();

        // Check terminal contents
        let styles = ViewContext::styles();
        harness.assert_buffer_lines([
            Line::styled("Species", styles.form.title_highlight),
            Line::styled("holy sh", Style::default()),
            Line::styled("it's a ", styles.list.highlight),
            Line::styled("look at", Style::default()),
            // Footer gets cut off
            Line::styled("Change ", styles.text.hint),
        ]);

        // Submit; submission event is handled by the ModalQueue. It's easier
        // just to call it manually since we test proper submission elsewhere
        component.into_inner().submit();
        assert_eq!(species_rx.try_recv().unwrap(), 2.into());
    }

    /// Add a text prompt to the form
    fn text(
        form: &mut PromptForm,
        message: &str,
        default: Option<&str>,
        sensitive: bool,
    ) -> Receiver<String> {
        let (tx, rx) = oneshot::channel();
        form.add_prompt(Prompt::Text {
            message: message.into(),
            default: default.map(String::from),
            sensitive,
            channel: tx.into(),
        });
        rx
    }

    /// Add a select prompt to the form
    fn select(
        form: &mut PromptForm,
        message: &str,
        options: Vec<(&str, Value)>,
    ) -> Receiver<Value> {
        let (tx, rx) = oneshot::channel();
        form.add_prompt(Prompt::Select {
            message: message.into(),
            options: options
                .into_iter()
                .map(|(label, value)| SelectOption {
                    label: label.into(),
                    value,
                })
                .collect(),
            channel: tx.into(),
        });
        rx
    }
}
