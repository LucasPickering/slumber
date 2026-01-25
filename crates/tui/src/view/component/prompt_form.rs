use crate::{
    http::{PromptId, PromptReply},
    message::HttpMessage,
    view::{
        Generate, UpdateContext, ViewContext,
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
        persistent::{PersistentStore, SessionKey},
    },
};
use indexmap::IndexMap;
use ratatui::{
    layout::{Constraint, Layout, Spacing},
    prelude::{Buffer, Rect},
    text::{Line, Span, Text},
    widgets::Widget,
};
use slumber_config::Action;
use slumber_core::{
    http::RequestId,
    render::{Prompt, SelectOption},
};
use std::{borrow::Cow, cmp, mem};
use tracing::error;

/// A form displaying prompts for the recipe builder
///
/// The TUI implementation of [Prompter](slumber_core::render::Prompter) sends
/// prompts here via the message queue. Whenever this has at least one prompt,
/// it should be shown. When the form is submitted, all prompts are submitted
/// together, clearing the queue.
#[derive(Debug)]
pub struct PromptForm {
    id: ComponentId,
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
    pub fn new(
        request_id: RequestId,
        prompts: &IndexMap<PromptId, Prompt>,
    ) -> Self {
        let inputs = prompts
            .iter()
            .map(|(id, prompt)| PromptInput::new(*id, prompt))
            .collect();
        Self {
            id: ComponentId::new(),
            request_id,
            select: Select::builder(inputs).build().into(),
            editing: true,
        }
    }

    pub fn request_id(&self) -> RequestId {
        self.request_id
    }

    /// Send a message with a reply for every prompt in the form
    fn submit(&mut self, store: &mut PersistentStore) {
        // We can take the select list without cloning, because this component
        // will be trashed on the update triggered by the message we send. This
        // is much simpler than using an emitted message to do this submission
        // in the parent, where the entire component is actually trashed.
        let select = mem::take(&mut self.select);
        let replies: Vec<(PromptId, PromptReply)> = select
            .into_select()
            .into_items()
            .map(|input| (input.prompt_id(), input.into_reply()))
            .collect();

        // Clear these values from the session store
        for (prompt_id, _) in &replies {
            store.remove_session(prompt_id);
        }

        ViewContext::send_message(HttpMessage::FormSubmit {
            request_id: self.request_id,
            replies,
        });
    }
}

impl Component for PromptForm {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn update(
        &mut self,
        context: &mut UpdateContext,
        event: Event,
    ) -> EventMatch {
        event.m().action(|action, propagate| match action {
            Action::PreviousPane => self.select.up(),
            Action::NextPane => self.select.down(),
            Action::Edit if !self.editing => self.editing = true,
            // If not editing, we'll propagate this to cancel the request
            Action::Cancel if self.editing => self.editing = false,
            Action::Submit => self.submit(context.persistent_store),
            _ => propagate.set(),
        })
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        vec![self.select.to_child_mut()]
    }
}

impl Draw for PromptForm {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        if self.select.is_empty() {
            // No prompts visible
            canvas.render_widget("Building...", metadata.area());
            return;
        }

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
        /// Use this to correlate the submission to the original prompt
        prompt_id: PromptId,
        message: String,
        text_box: TextBox,
    },
    /// Prompt the user to select an item from a list
    Select {
        id: ComponentId,
        /// Use this to correlate the submission to the original prompt
        prompt_id: PromptId,
        message: String,
        /// List of options to present to the user
        select: Select<SelectOption>,
    },
}

impl PromptInput {
    fn new(prompt_id: PromptId, prompt: &Prompt) -> Self {
        let persisted = PersistentStore::get_session(&prompt_id);

        match prompt {
            Prompt::Text {
                message,
                default,
                sensitive,
                ..
            } => {
                // If we have a persisted value from a previous life, use it
                let default = persisted
                    .and_then(PromptValue::into_text)
                    // Otherwise use the default
                    .or_else(|| default.clone())
                    .unwrap_or_default();
                Self::Text {
                    id: ComponentId::default(),
                    prompt_id,
                    message: message.clone(),
                    text_box: TextBox::default()
                        .sensitive(*sensitive)
                        .default_value(default),
                }
            }
            Prompt::Select {
                message, options, ..
            } => Self::Select {
                id: ComponentId::default(),
                prompt_id,
                message: message.clone(),
                select: Select::builder(options.clone())
                    .preselect_index(
                        // Preselect index from a previous life
                        persisted
                            .and_then(PromptValue::into_select)
                            .unwrap_or(0),
                    )
                    .build(),
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

    /// Unique ID for this prompt, which will tie it back to the request store
    fn prompt_id(&self) -> PromptId {
        match self {
            PromptInput::Text { prompt_id, .. }
            | PromptInput::Select { prompt_id, .. } => *prompt_id,
        }
    }

    /// Extract the current value as a reply to be sent back to the request
    /// store.
    fn into_reply(self) -> PromptReply {
        match self {
            Self::Text { text_box, .. } => {
                PromptReply::Text(text_box.into_text())
            }
            Self::Select { select, .. } => {
                // Non-empty select is enforced by the select() function
                let option =
                    select.into_selected().expect("Select cannot be empty");
                PromptReply::Select(option.value)
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

    fn persist(&self, store: &mut PersistentStore) {
        // Prompt values are persisted **within a single session**. There's no
        // reason to persist across sessions because any unbuilt requests will
        // be deleted when the program exits
        let value = match self {
            Self::Text { text_box, .. } => {
                Some(PromptValue::Text(text_box.text().to_owned()))
            }
            Self::Select { select, .. } => {
                select.selected_index().map(PromptValue::Select)
            }
        };
        if let Some(value) = value {
            store.set_session(self.prompt_id(), value);
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

/// Persist incomplete prompt responses in the session store
impl SessionKey for PromptId {
    type Value = PromptValue;
}

/// Persisted value for a prompt
#[derive(Clone, Debug, PartialEq)]
pub enum PromptValue {
    Text(String),
    Select(usize),
}

impl PromptValue {
    /// Extract a text prompt value
    fn into_text(self) -> Option<String> {
        match self {
            PromptValue::Text(text) => Some(text),
            PromptValue::Select(_) => {
                // Prompts can't change type, so this indicates a bug
                error!(?self, "Incorrect prompt type; expected text");
                None
            }
        }
    }

    /// Extract a selected index prompt value
    fn into_select(self) -> Option<usize> {
        match self {
            PromptValue::Select(index) => Some(index),
            PromptValue::Text(_) => {
                // Prompts can't change type, so this indicates a bug
                error!(?self, "Incorrect prompt type; expected select");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        message::Message,
        test_util::{TestTerminal, terminal},
        view::test_util::{TestComponent, TestHarness, harness},
    };
    use itertools::Itertools;
    use ratatui::style::{Style, Styled};
    use rstest::rstest;
    use slumber_template::Value;
    use slumber_util::assert_matches;
    use terminput::{KeyCode, KeyModifiers};
    use tokio::sync::oneshot;

    /// Navigate between multiple fields and submit
    #[rstest]
    fn test_navigation(mut harness: TestHarness, terminal: TestTerminal) {
        let request_id = RequestId::new();
        let prompts = IndexMap::from_iter([
            text("Username", Some("user"), false),
            select(
                "Species",
                vec![
                    ("holy shit what is that thing", 1.into()),
                    ("it's a baby fuckin wheel!", 2.into()),
                    ("look at that thing jay!", 3.into()),
                ],
            ),
            text("Password", Some("hunter2"), true),
        ]);
        let mut component = TestComponent::new(
            &harness,
            &terminal,
            PromptForm::new(request_id, &prompts),
        );

        component
            .int()
            .drain_draw() // Draw so children are visible
            .send_text("123") // Modify username
            .inspect(|component| {
                assert!(component.editing);
                assert_eq!(component.select.selected_index(), Some(0));
            })
            .send_key(KeyCode::Tab) // Switch to species - still editing
            .send_key(KeyCode::Down) // Select 2nd option
            // Exit edit mode, nav w/ arrow keys, then re-enter edit
            .send_keys([KeyCode::Esc, KeyCode::Up, KeyCode::Char('e')])
            .send_text("4") // Modify username again
            .send_key_modifiers(KeyCode::Up, KeyModifiers::SHIFT) // Wrap to pw
            .send_text("456") // Modify password
            .send_key(KeyCode::Enter) // Submit
            .assert()
            .empty();

        let (actual_request_id, replies) = assert_matches!(
            harness.messages().pop_now(),
            Message::Http(HttpMessage::FormSubmit {
                request_id,
                replies,
            }) => (request_id, replies)
        );
        assert_eq!(actual_request_id, request_id);
        let prompt_ids = prompts.keys().copied().collect_vec();
        assert_eq!(
            replies,
            vec![
                (prompt_ids[0], PromptReply::Text("user1234".into())),
                (prompt_ids[1], PromptReply::Select(2.into())),
                (prompt_ids[2], PromptReply::Text("hunter2456".into())),
            ]
        );
    }

    /// If you open a prompt, edit the value, then navigate away from the form
    /// without submitting, the values should be persisted. This is possible
    /// if you change requests, change view, etc. and the prompt form is
    /// hidden temporarily. We should retain the form state when the user
    /// navigates back
    #[rstest]
    fn test_persistence(
        mut harness: TestHarness,
        #[with(8, 2)] terminal: TestTerminal,
    ) {
        let request_id = RequestId::new();
        let prompts = IndexMap::from_iter([
            text("Text", None, false),
            select("Select", vec![("a", 0.into()), ("b", 1.into())]),
        ]);
        let mut component = TestComponent::new(
            &harness,
            &terminal,
            PromptForm::new(request_id, &prompts),
        );

        // Test every kind of prompt
        component
            .int()
            .send_text("user") // Enter username
            .send_key(KeyCode::Tab) // Switch to Select
            .send_key(KeyCode::Down) // Select second item
            .assert()
            .empty();

        // Values should be in the session store
        assert_eq!(
            PersistentStore::get_session(&prompts.keys()[0]),
            Some(PromptValue::Text("user".into()))
        );
        assert_eq!(
            PersistentStore::get_session(&prompts.keys()[1]),
            Some(PromptValue::Select(1))
        );

        // Rebuild the component and the values are restored
        let mut component = TestComponent::new(
            &harness,
            &terminal,
            PromptForm::new(request_id, &prompts),
        );
        component.int().send_key(KeyCode::Enter).assert().empty();

        // Our previous values were submitted
        let replies = assert_matches!(
            harness.messages().pop_now(),
            Message::Http(HttpMessage::FormSubmit {
                replies,
                ..
            }) => replies
        );
        let prompt_ids = prompts.keys().copied().collect_vec();
        assert_eq!(
            replies,
            vec![
                (prompt_ids[0], PromptReply::Text("user".into())),
                (prompt_ids[1], PromptReply::Select(1.into())),
            ]
        );

        // Values were cleared out of the session store
        assert_eq!(PersistentStore::get_session(&prompts.keys()[0]), None);
        assert_eq!(PersistentStore::get_session(&prompts.keys()[1]), None);
    }

    /// Text input field
    #[rstest]
    fn test_text(
        mut harness: TestHarness,
        #[with(8, 5)] terminal: TestTerminal,
    ) {
        let prompts = IndexMap::from_iter([
            text("Username", Some("user"), false),
            text("Password", Some("hunter"), true),
        ]);
        let mut component = TestComponent::new(
            &harness,
            &terminal,
            PromptForm::new(RequestId::new(), &prompts),
        );

        component
            .int()
            .drain_draw() // Draw so children are visible
            .send_text("12") // Modify username
            .send_key(KeyCode::Tab) // Switch to password
            .send_text("2") // Modify password
            .assert()
            .empty();

        // Check terminal contents
        let styles = ViewContext::styles();
        terminal.assert_buffer_lines([
            Line::styled("Username", styles.form.title),
            Line::styled("user12  ", styles.form.content),
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
            .assert()
            .empty();
        let replies = assert_matches!(
            harness.messages().pop_now(),
            Message::Http(HttpMessage::FormSubmit {
                request_id,
                replies,
            }) => replies
        );
        let prompt_ids = prompts.keys().copied().collect_vec();
        assert_eq!(
            replies,
            vec![
                (prompt_ids[0], PromptReply::Text("user12".into())),
                (prompt_ids[1], PromptReply::Text("hunter2".into())),
            ]
        );
    }

    /// Select input field
    #[rstest]
    fn test_select(
        mut harness: TestHarness,
        #[with(7, 5)] terminal: TestTerminal,
    ) {
        let prompts = IndexMap::from_iter([select(
            "Species",
            vec![
                ("holy shit what is that thing", 1.into()),
                ("it's a baby fuckin wheel!", 2.into()),
                ("look at that thing jay!", 3.into()),
            ],
        )]);
        let mut component = TestComponent::new(
            &harness,
            &terminal,
            PromptForm::new(RequestId::new(), &prompts),
        );

        component
            .int()
            .drain_draw() // Draw so children are visible
            .send_key(KeyCode::Down)
            .assert()
            .empty();

        // Check terminal contents
        let styles = ViewContext::styles();
        terminal.assert_buffer_lines([
            Line::styled("Species", styles.form.title_highlight),
            Line::styled("holy sh", Style::default()),
            Line::styled("it's a ", styles.list.highlight),
            Line::styled("look at", Style::default()),
            // Footer gets cut off
            Line::styled("Change ", styles.text.hint),
        ]);

        // Submit
        component.int().send_key(KeyCode::Enter).assert().empty();
        let replies = assert_matches!(
            harness.messages().pop_now(),
            Message::Http(HttpMessage::FormSubmit {
                request_id,
                replies,
            }) => replies
        );
        let prompt_ids = prompts.keys().copied().collect_vec();
        assert_eq!(
            replies,
            vec![(prompt_ids[0], PromptReply::Select(2.into()))]
        );
    }

    /// Create a text prompt
    fn text(
        message: &str,
        default: Option<&str>,
        sensitive: bool,
    ) -> (PromptId, Prompt) {
        let (tx, _) = oneshot::channel();
        let prompt = Prompt::Text {
            message: message.into(),
            default: default.map(String::from),
            sensitive,
            channel: tx.into(),
        };
        (PromptId::new(), prompt)
    }

    /// Create a select prompt
    fn select(
        message: &str,
        options: Vec<(&str, Value)>,
    ) -> (PromptId, Prompt) {
        let (tx, _) = oneshot::channel();
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
        (PromptId::new(), prompt)
    }
}
