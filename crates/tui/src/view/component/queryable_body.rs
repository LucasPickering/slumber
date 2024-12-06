//! Request/response body display component

use crate::{
    context::TuiContext,
    util::run_command,
    view::{
        common::{
            text_box::{TextBox, TextBoxEvent},
            text_window::{ScrollbarMargins, TextWindow, TextWindowProps},
        },
        context::UpdateContext,
        draw::{Draw, DrawMetadata},
        event::{Child, Emitter, EmitterId, Event, EventHandler, Update},
        state::Identified,
        util::{highlight, str_to_text},
        Component,
    },
};
use persisted::PersistedContainer;
use ratatui::{
    layout::{Constraint, Layout},
    text::Text,
    Frame,
};
use slumber_config::Action;
use slumber_core::{
    http::{content_type::ContentType, ResponseBody, ResponseRecord},
    util::MaybeStr,
};
use std::sync::Arc;
use tokio::task;

/// Display response body as text, with a query box to run commands on the body.
/// The query state can be persisted by persisting this entire container.
#[derive(Debug)]
pub struct QueryableBody {
    emitter_id: EmitterId,
    response: Arc<ResponseRecord>,

    /// Are we currently typing in the query box?
    query_focused: bool,
    /// Shell command used to transform the content body
    query_command: Option<String>,
    /// Where the user enters their body query
    query_text_box: Component<TextBox>,
    /// Filtered text display
    text_window: Component<TextWindow>,

    /// Data that can update as the query changes
    state: TextState,
}

impl QueryableBody {
    /// Create a new body, optionally loading the query text from the
    /// persistence DB. This is optional because not all callers use the query
    /// box, or want to persist the value.
    pub fn new(response: Arc<ResponseRecord>) -> Self {
        let input_engine = &TuiContext::get().input_engine;
        let binding = input_engine.binding_display(Action::Search);

        let text_box = TextBox::default()
            .placeholder(format!("{binding} to filter"))
            .placeholder_focused("Enter command (ex: `jq .results`)")
            .debounce();
        let state = init_state(response.content_type(), &response.body);

        Self {
            emitter_id: EmitterId::new(),
            response,
            query_focused: false,
            query_command: None,
            query_text_box: text_box.into(),
            text_window: Default::default(),
            state,
        }
    }

    /// If a query command has been applied, get the visible text. Otherwise,
    /// return `None` to indicate the response's original body can be used.
    /// Return an owned value because we have to join the text to a string.
    pub fn modified_text(&self) -> Option<String> {
        if self.query_command.is_none() {
            None
        } else {
            Some(self.state.text.to_string())
        }
    }

    /// Get visible body text
    pub fn visible_text(&self) -> &Text {
        &self.state.text
    }

    /// Update query command based on the current text in the box, and start
    /// a task to run the command
    fn update_query(&mut self) {
        let command = self.query_text_box.data().text();
        if command.is_empty() {
            // Reset to initial body
            self.query_command = None;
            self.state =
                init_state(self.response.content_type(), &self.response.body);
        } else {
            self.query_command = Some(command.to_owned());

            // Spawn the command in the background because it could be slow.
            // Clone is cheap because Bytes uses refcounting
            let body = self.response.body.bytes().clone();
            let command = command.to_owned();
            let emitter = self.detach();
            task::spawn_local(async move {
                let result = run_command(&command, Some(&body)).await;
                emitter.emit(QueryComplete(result));
            });
        }
    }
}

impl EventHandler for QueryableBody {
    fn update(&mut self, _: &mut UpdateContext, event: Event) -> Update {
        if let Some(Action::Search) = event.action() {
            self.query_focused = true;
        } else if let Some(event) = self.query_text_box.emitted(&event) {
            match event {
                TextBoxEvent::Focus => self.query_focused = true,
                TextBoxEvent::Change => self.update_query(),
                TextBoxEvent::Cancel => {
                    // Reset text to whatever was submitted last
                    self.query_text_box.data_mut().set_text(
                        self.query_command.clone().unwrap_or_default(),
                    );
                    self.query_focused = false;
                }
                TextBoxEvent::Submit => {
                    self.update_query();
                    self.query_focused = false;
                }
            }
        } else if let Some(QueryComplete(result)) = self.emitted(&event) {
            match result {
                Ok(stdout) => {
                    self.state = init_state(
                        // Assume the output has the same content type
                        self.response.content_type(),
                        &ResponseBody::new(stdout),
                    );
                }
                // Trigger error state. We DON'T want to show a modal here
                // because it's incredibly annoying. Maybe there should be a
                // way to open the error though?
                Err(_) => self.query_text_box.data_mut().set_error(),
            }
        } else {
            return Update::Propagate(event);
        }
        Update::Consumed
    }

    fn children(&mut self) -> Vec<Component<Child<'_>>> {
        vec![
            self.query_text_box.to_child_mut(),
            self.text_window.to_child_mut(),
        ]
    }
}

impl Draw for QueryableBody {
    fn draw(&self, frame: &mut Frame, _: (), metadata: DrawMetadata) {
        let [body_area, query_area] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(1)])
                .areas(metadata.area());

        self.text_window.draw(
            frame,
            TextWindowProps {
                text: &self.state.text,
                margins: ScrollbarMargins {
                    bottom: 2, // Extra margin to jump over the search box
                    ..Default::default()
                },
                footer: None,
            },
            body_area,
            true,
        );

        self.query_text_box
            .draw(frame, (), query_area, self.query_focused);
    }
}

/// Persist the query text box
impl PersistedContainer for QueryableBody {
    type Value = String;

    fn get_to_persist(&self) -> Self::Value {
        self.query_text_box.data().get_to_persist()
    }

    fn restore_persisted(&mut self, value: Self::Value) {
        self.query_text_box.data_mut().restore_persisted(value)
    }
}

impl Emitter for QueryableBody {
    type Emitted = QueryComplete;

    fn id(&self) -> EmitterId {
        self.emitter_id
    }
}

#[derive(Debug)]
struct TextState {
    /// The full body, which we need to track for launching commands
    text: Identified<Text<'static>>,
}

/// Emitted event to notify when a query subprocess has completed. Contains the
/// stdout of the process if successful.
#[derive(Debug)]
pub struct QueryComplete(anyhow::Result<Vec<u8>>);

/// Calculate display text based on current body/query
fn init_state<T: AsRef<[u8]>>(
    content_type: Option<ContentType>,
    body: &ResponseBody<T>,
) -> TextState {
    if TuiContext::get().config.http.is_large(body.size()) {
        // For bodies over the "large" size, skip prettification and
        // highlighting because it's slow. We could try to push this work
        // into a background thread instead, but there's no way to kill those
        // threads so we could end up piling up a lot of work. It also burns
        // a lot of CPU, regardless of where it's run
        //
        // We don't show a hint to the user in this case because it's not
        // worth the screen real estate
        if let Some(text) = body.text() {
            TextState {
                text: str_to_text(text).into(),
            }
        } else {
            // Showing binary content is a bit of a novelty, there's not much
            // value in it. For large bodies it's not worth the CPU cycles
            let text: Text = "<binary>".into();
            TextState { text: text.into() }
        }
    } else if let Some(text) = body.text() {
        let text = highlight::highlight_if(content_type, str_to_text(text));
        TextState { text: text.into() }
    } else {
        // Content is binary, show a textual representation of it
        let text: Text =
            format!("{:#}", MaybeStr(body.bytes().as_ref())).into();
        TextState { text: text.into() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        context::TuiContext,
        test_util::{harness, run_local, terminal, TestHarness, TestTerminal},
        view::{
            test_util::TestComponent,
            util::persistence::{DatabasePersistedStore, PersistedLazy},
        },
    };
    use crossterm::event::KeyCode;
    use persisted::{PersistedKey, PersistedStore};
    use ratatui::text::Span;
    use reqwest::StatusCode;
    use rstest::{fixture, rstest};
    use serde::Serialize;
    use slumber_core::http::{ResponseBody, ResponseRecord};

    const TEXT: &[u8] = b"{\"greeting\":\"hello\"}";

    /// Style text to match the text window gutter
    fn gutter(text: &str) -> Span {
        let styles = &TuiContext::get().styles;
        Span::styled(text, styles.text_window.gutter)
    }

    #[fixture]
    fn response() -> Arc<ResponseRecord> {
        ResponseRecord {
            status: StatusCode::OK,
            // Note: do NOT set the content-type header here. All it does is
            // enable syntax highlighting, which makes buffer assertions hard
            headers: Default::default(),
            body: ResponseBody::new(TEXT.into()),
        }
        .into()
    }

    /// Render a text body with query text box
    #[rstest]
    #[tokio::test]
    async fn test_text_body(
        harness: TestHarness,
        #[with(26, 3)] terminal: TestTerminal,
        response: Arc<ResponseRecord>,
    ) {
        let mut component = TestComponent::new(
            &harness,
            &terminal,
            QueryableBody::new(response),
            (),
        );

        // Assert initial state/view
        let data = component.data();
        assert_eq!(data.query_command, None);
        assert_eq!(data.modified_text().as_deref(), None);
        let styles = &TuiContext::get().styles.text_box;
        terminal.assert_buffer_lines([
            vec![gutter("1"), " {\"greeting\":\"hello\"}".into()],
            vec![gutter(" "), "                       ".into()],
            vec![
                Span::styled(
                    "/ to filter",
                    styles.text.patch(styles.placeholder),
                ),
                Span::styled("               ", styles.text),
            ],
        ]);

        // Type something into the query box
        component.send_key(KeyCode::Char('/')).assert_empty();
        // Both the debounce and the subprocess use local tasks, so we need to
        // run in a local set. When this future exits, all tasks are done
        run_local(async {
            component.send_text("head -c 1").assert_empty();
            component.send_key(KeyCode::Enter).assert_empty();
        })
        .await;
        // Command is done, handle its resulting event
        component.drain_draw().assert_empty();

        // Make sure state updated correctly
        let data = component.data();
        assert_eq!(data.query_command.as_deref(), Some("head -c 1"));
        assert_eq!(data.modified_text().as_deref(), Some("{"));
        assert!(!data.query_focused);

        // Cancelling out of the text box should reset the query value
        component.send_key(KeyCode::Char('/')).assert_empty();
        run_local(async {
            // Local task needed for the debounce
            component.send_text("more text").assert_empty();
            component.send_key(KeyCode::Esc).assert_empty();
        })
        .await;
        let data = component.data();
        assert_eq!(data.query_command.as_deref(), Some("head -c 1"));
        assert_eq!(data.query_text_box.data().text(), "head -c 1");
        assert!(!data.query_focused);

        // Check the view again
        terminal.assert_buffer_lines([
            vec![gutter("1"), " {                  ".into()],
            vec![gutter(" "), "                    ".into()],
            vec![Span::styled("head -c 1                 ", styles.text)],
        ]);
    }

    /// Render a parsed body with query text box, and load initial query from
    /// the DB. This tests the `PersistedContainer` implementation
    #[rstest]
    #[tokio::test]
    async fn test_persistence(
        harness: TestHarness,
        #[with(30, 4)] terminal: TestTerminal,
        response: Arc<ResponseRecord>,
    ) {
        #[derive(Debug, Serialize, PersistedKey)]
        #[persisted(String)]
        struct Key;

        // Add initial query to the DB
        DatabasePersistedStore::store_persisted(&Key, &"head -n 1".to_owned());

        let mut component = TestComponent::new(
            &harness,
            &terminal,
            PersistedLazy::new(Key, QueryableBody::new(response)),
            (),
        );

        // We already have another test to check that querying works via typing
        // in the box, so we just need to make sure state is initialized
        // correctly here. Command execution requires a localset
        run_local(async {
            component.drain_draw().assert_empty();
        })
        .await;
        assert_eq!(
            component.data().query_command.as_deref(),
            Some("head -n 1")
        );
    }
}
