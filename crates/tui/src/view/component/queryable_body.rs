//! Request/response body display component

use crate::{
    context::TuiContext,
    util,
    view::{
        Component, IntoModal, ViewContext,
        common::{
            modal::Modal,
            text_box::{TextBox, TextBoxEvent, TextBoxProps},
            text_window::{ScrollbarMargins, TextWindow, TextWindowProps},
        },
        context::UpdateContext,
        draw::{Draw, DrawMetadata, Generate},
        event::{Child, Emitter, Event, EventHandler, OptionEvent, ToEmitter},
        state::Identified,
        util::{highlight, str_to_text},
    },
};
use anyhow::Context;
use bytes::Bytes;
use persisted::PersistedContainer;
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    text::Text,
};
use slumber_config::Action;
use slumber_core::{
    http::{ResponseBody, ResponseRecord, content_type::ContentType},
    util::MaybeStr,
};
use std::{borrow::Cow, mem, sync::Arc};
use tokio::task::AbortHandle;

/// Display response body as text, with a query box to run commands on the body.
/// The query state can be persisted by persisting this entire container.
#[derive(Debug)]
pub struct QueryableBody {
    emitter: Emitter<QueryComplete>,
    response: Arc<ResponseRecord>,

    /// Which command box, if any, are we typing in?
    command_focus: CommandFocus,
    /// Default query to use when none is present. We have to store this so we
    /// can apply it when an empty query is loaded from persistence. Generally
    /// this will come from the config but it's parameterized for testing
    default_query: Option<String>,
    /// Track status of the current query command
    query_state: QueryState,
    /// Where the user enters their body query
    query_text_box: Component<TextBox>,
    /// Query command to reset back to when the user hits cancel
    last_executed_query: Option<String>,

    /// Export command, for side effects. This isn't persistent, so the state
    /// is a lot simpler. We'll clear this out whenever the user exits.
    export_text_box: Component<TextBox>,

    /// Filtered text display
    text_window: Component<TextWindow>,

    /// Data that can update as the query changes
    text_state: TextState,
}

impl QueryableBody {
    /// Create a new body, optionally loading the query text from the
    /// persistence DB. This is optional because not all callers use the query
    /// box, or want to persist the value.
    pub fn new(
        response: Arc<ResponseRecord>,
        default_query: Option<String>,
    ) -> Self {
        let input_engine = &TuiContext::get().input_engine;
        let query_bind = input_engine.binding_display(Action::Search);
        let export_bind = input_engine.binding_display(Action::Export);

        let query_text_box = TextBox::default()
            .placeholder(format!(
                "{query_bind} to query, {export_bind} to export"
            ))
            .placeholder_focused("Enter query command (ex: `jq .results`)")
            .default_value(default_query.clone().unwrap_or_default());
        let export_text_box = TextBox::default().placeholder_focused(
            "Enter export command (ex: `tee > response.json`)",
        );

        let text_state =
            TextState::new(response.content_type(), &response.body, true);

        let mut slf = Self {
            emitter: Default::default(),
            response,
            command_focus: CommandFocus::None,
            default_query,
            query_state: QueryState::None,
            query_text_box: query_text_box.into(),
            last_executed_query: None,
            export_text_box: export_text_box.into(),
            text_window: Default::default(),
            text_state,
        };
        // If we have an initial query from the default value, run it now
        slf.update_query();
        slf
    }

    /// If the original body text is _not_ what the user is looking at (because
    /// of a query command or prettification), get the visible text. Otherwise,
    /// return `None` to indicate the response's original body can be used.
    /// Binary bodies will return `None` here. Return an owned value because we
    /// have to join the text to a string.
    pub fn modified_text(&self) -> Option<String> {
        if matches!(self.query_state, QueryState::Ok) || self.text_state.pretty
        {
            Some(self.text_state.text.to_string())
        } else {
            None
        }
    }

    /// Get whatever text the user sees
    pub fn visible_text(&self) -> &Text {
        &self.text_state.text
    }

    fn focus(&mut self, focus: CommandFocus) {
        self.command_focus = focus;
    }

    /// Update query command based on the current text in the box, and start
    /// a task to run the command
    fn update_query(&mut self) {
        let command = self.query_text_box.data().text().trim();

        // If the command hasn't changed, do nothing
        if self.last_executed_query.as_deref() == Some(command) {
            return;
        }

        // If a different command is already running, abort it
        if let Some(handle) = self.query_state.take_abort_handle() {
            handle.abort();
        }

        if command.is_empty() {
            // Reset to initial body
            self.last_executed_query = None;
            self.query_state = QueryState::None;
            self.text_state = TextState::new(
                self.response.content_type(),
                &self.response.body,
                true, // Prettify
            );
        } else {
            // Send it
            self.last_executed_query = Some(command.to_owned());

            // Spawn the command in the background because it could be slow.
            // Clone is cheap because Bytes uses refcounting
            let body = self.response.body.bytes().clone();
            let command = command.to_owned();
            let emitter = self.emitter;
            let abort_handle =
                self.spawn_command(command, body, move |_, result| {
                    emitter.emit(QueryComplete(result));
                });
            self.query_state = QueryState::Running(abort_handle);
        }
    }

    /// Run an export shell command with the response as stdin. The output
    /// will *not* be reflected in the UI. Used for things like saving a
    /// response to a file.
    fn export(&mut self) {
        let command = self.export_text_box.data_mut().clear();

        if command.is_empty() {
            return;
        }

        // If text has been modified by formatting/query, pass that to stdin.
        // For unadulterated bodies, use the original. For large and/or binary
        // bodies we'll just clone the Bytes object, which is cheap because it
        // uses refcounting
        let body = self
            .modified_text()
            .map(Bytes::from)
            .unwrap_or_else(|| self.response.body.bytes().clone());

        self.spawn_command(command, body, |command, result| match result {
            // We provide feedback via a global mechanism in both cases, so we
            // don't need an emitter here
            Ok(_) => ViewContext::notify(format!("`{command}` succeeded")),
            Err(error) => error.into_modal().open(),
        });
    }

    /// Run a shell command in a background task
    fn spawn_command(
        &self,
        command: String,
        body: Bytes,
        on_complete: impl 'static + FnOnce(String, anyhow::Result<Vec<u8>>),
    ) -> AbortHandle {
        util::spawn("spawn_command", async move {
            let shell = &TuiContext::get().config.commands.shell;
            let result = util::run_command(shell, &command, Some(&body))
                .await
                .with_context(|| format!("Error running `{command}`"));
            on_complete(command, result);
        })
        .abort_handle()
    }
}

impl EventHandler for QueryableBody {
    fn update(&mut self, _: &mut UpdateContext, event: Event) -> Option<Event> {
        event
            .opt()
            .action(|action, propagate| match action {
                Action::Search => self.focus(CommandFocus::Query),
                Action::Export => self.focus(CommandFocus::Export),
                _ => propagate.set(),
            })
            .emitted(self.emitter, |QueryComplete(result)| match result {
                Ok(stdout) => {
                    self.query_state = QueryState::Ok;
                    self.text_state = TextState::new(
                        // Assume the output has the same content type
                        self.response.content_type(),
                        &ResponseBody::new(stdout),
                        // Don't prettify - user controls this output. If
                        // it's not pretty already, that's on them
                        false,
                    );
                }
                // Trigger error state. Error will be shown in the pane
                Err(error) => self.query_state = QueryState::Error(error),
            })
            .emitted(self.query_text_box.to_emitter(), |event| match event {
                TextBoxEvent::Focus => self.focus(CommandFocus::Query),
                TextBoxEvent::Change => {}
                TextBoxEvent::Cancel => {
                    // Reset text to whatever was submitted last
                    self.query_text_box.data_mut().set_text(
                        self.last_executed_query.clone().unwrap_or_default(),
                    );
                    self.focus(CommandFocus::None);
                }
                TextBoxEvent::Submit => {
                    self.update_query();
                    self.focus(CommandFocus::None);
                }
            })
            .emitted(self.export_text_box.to_emitter(), |event| match event {
                TextBoxEvent::Focus => self.focus(CommandFocus::Export),
                TextBoxEvent::Change => {}
                TextBoxEvent::Cancel => {
                    self.export_text_box.data_mut().clear();
                    self.focus(CommandFocus::None);
                }
                TextBoxEvent::Submit => {
                    self.export();
                    self.focus(CommandFocus::None);
                }
            })
    }

    fn children(&mut self) -> Vec<Component<Child<'_>>> {
        vec![
            self.query_text_box.to_child_mut(),
            self.export_text_box.to_child_mut(),
            self.text_window.to_child_mut(),
        ]
    }
}

impl Draw for QueryableBody {
    fn draw(&self, frame: &mut Frame, (): (), metadata: DrawMetadata) {
        let [body_area, query_area] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(1)])
                .areas(metadata.area());

        if let QueryState::Error(error) = &self.query_state {
            frame.render_widget(error.generate(), body_area);
        } else {
            self.text_window.draw(
                frame,
                TextWindowProps {
                    text: &self.text_state.text,
                    margins: ScrollbarMargins {
                        bottom: 2, // Extra margin to jump over the search box
                        ..Default::default()
                    },
                },
                body_area,
                true,
            );
        }

        // Only show the export box when focused, otherwise show query
        if self.command_focus == CommandFocus::Export {
            self.export_text_box.draw(
                frame,
                TextBoxProps::default(),
                query_area,
                true,
            );
        } else {
            self.query_text_box.draw(
                frame,
                TextBoxProps {
                    has_error: matches!(self.query_state, QueryState::Error(_)),
                },
                query_area,
                self.command_focus == CommandFocus::Query,
            );
        }
    }
}

/// Persist the query text box
impl PersistedContainer for QueryableBody {
    type Value = String;

    fn get_to_persist(&self) -> Self::Value {
        self.query_text_box.data().get_to_persist()
    }

    fn restore_persisted(&mut self, value: Self::Value) {
        let text_box = self.query_text_box.data_mut();
        text_box.restore_persisted(value);

        // It's pretty common to clear the whole text box without thinking about
        // it. In that case, we want to restore the default the next time we
        // reload from persistence (probably either app restart or next response
        // for this recipe). It's possible the user really wants an empty box
        // and this is annoying, but I think it'll be more good than bad.
        if text_box.text().is_empty() {
            if let Some(query) = self.default_query.clone() {
                self.query_text_box.data_mut().set_text(query);
            }
        }

        // Update local state and execute the query command (if any)
        self.update_query();
    }
}

impl ToEmitter<QueryComplete> for QueryableBody {
    fn to_emitter(&self) -> Emitter<QueryComplete> {
        self.emitter
    }
}

#[derive(Debug)]
struct TextState {
    /// The full body, which we need to track for launching commands
    text: Identified<Text<'static>>,
    /// Was the text prettified? We track this so we know if we've modified the
    /// original text
    pretty: bool,
}

impl TextState {
    /// Calculate display text based on current body/query
    fn new<T: AsRef<[u8]>>(
        content_type: Option<ContentType>,
        body: &ResponseBody<T>,
        prettify: bool,
    ) -> Self {
        if TuiContext::get().config.http.is_large(body.size()) {
            // For bodies over the "large" size, skip prettification and
            // highlighting because it's slow. We could try to push this work
            // into a background thread instead, but there's no way to kill
            // those threads so we could end up piling up a lot of work. It also
            // burns a lot of CPU, regardless of where it's run
            //
            // We don't show a hint to the user in this case because it's not
            // worth the screen real estate
            if let Some(text) = body.text() {
                TextState {
                    text: str_to_text(text).into(),
                    pretty: false,
                }
            } else {
                // Showing binary content is a bit of a novelty, there's not
                // much value in it. For large bodies it's not
                // worth the CPU cycles
                let text: Text = "<binary>".into();
                TextState {
                    text: text.into(),
                    pretty: false,
                }
            }
        } else if let Some(text) = body.text() {
            // Prettify for known content types. We _don't_ do this in a
            // separate task because it's generally very fast. If this is slow
            // enough that it affects the user, the "large" body size is
            // probably too low
            // unstable: if-let chain
            // https://github.com/rust-lang/rust/pull/132833
            let (text, pretty): (Cow<str>, bool) = match content_type {
                Some(content_type) if prettify => content_type
                    .prettify(text)
                    .map(|body| (Cow::Owned(body), true))
                    .unwrap_or((Cow::Borrowed(text), false)),
                _ => (Cow::Borrowed(text), false),
            };

            let text =
                highlight::highlight_if(content_type, str_to_text(&text));
            TextState {
                text: text.into(),
                pretty,
            }
        } else {
            // Content is binary, show a textual representation of it
            let text: Text =
                format!("{:#}", MaybeStr(body.bytes().as_ref())).into();
            TextState {
                text: text.into(),
                pretty: false,
            }
        }
    }
}

/// Which command box, if any, is focused?
#[derive(Copy, Clone, Debug, PartialEq)]
enum CommandFocus {
    None,
    Query,
    Export,
}

/// Emitted event to notify when a query subprocess has completed. Contains the
/// stdout of the process if successful.
#[derive(Debug)]
struct QueryComplete(Result<Vec<u8>, anyhow::Error>);

#[derive(Debug, Default)]
enum QueryState {
    /// Query has not been run yet
    #[default]
    None,
    /// Command is running. Handle can be used to kill it
    Running(AbortHandle),
    /// Command failed
    Error(anyhow::Error),
    // Success! The result is immediately transformed and stored in the text
    // window, so we don't need to store it here
    Ok,
}

impl QueryState {
    fn take_abort_handle(&mut self) -> Option<AbortHandle> {
        match mem::take(self) {
            Self::Running(handle) => Some(handle),
            other => {
                // Put it back!
                *self = other;
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        context::TuiContext,
        test_util::{TestHarness, TestTerminal, harness, run_local, terminal},
        view::{
            test_util::TestComponent,
            util::persistence::{DatabasePersistedStore, PersistedLazy},
        },
    };
    use crossterm::event::KeyCode;
    use persisted::{PersistedKey, PersistedStore};
    use ratatui::{layout::Margin, text::Span};
    use rstest::{fixture, rstest};
    use serde::Serialize;
    use slumber_core::http::{ResponseBody, ResponseRecord};
    use slumber_util::{Factory, TempDir, assert_matches, temp_dir};
    use tokio::fs;

    const TEXT: &str = "{\"greeting\":\"hello\"}";

    /// Persisted key for testing
    #[derive(Debug, Serialize, PersistedKey)]
    #[persisted(String)]
    struct Key;

    /// Style text to match the text window gutter
    fn gutter(text: &str) -> Span {
        let styles = &TuiContext::get().styles;
        Span::styled(text, styles.text_window.gutter)
    }

    #[fixture]
    fn response() -> Arc<ResponseRecord> {
        ResponseRecord {
            // Note: do NOT set the content-type header here. It enables syntax
            // highlighting, which makes buffer assertions hard. JSON-specific
            // behavior is tested in ResponseView
            headers: Default::default(),
            body: ResponseBody::new(TEXT.into()),
            ..ResponseRecord::factory(())
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
            QueryableBody::new(response, None),
        );

        // Assert initial state/view
        let data = component.data();
        assert_eq!(data.last_executed_query, None);
        assert_eq!(data.modified_text().as_deref(), None);
        let styles = &TuiContext::get().styles.text_box;
        terminal.assert_buffer_lines([
            vec![gutter("1"), " {\"greeting\":\"hello\"}".into()],
            vec![gutter(" "), "                       ".into()],
            vec![
                Span::styled(
                    "/ to query, : to export",
                    styles.text.patch(styles.placeholder),
                ),
                Span::styled("   ", styles.text),
            ],
        ]);

        // Type something into the query box
        component.int().send_key(KeyCode::Char('/')).assert_empty();
        // The subprocess uses local tasks, so we need to run in a local set.
        // When this future exits, all tasks are done
        run_local(async {
            component
                .int()
                .send_text("head -c 1")
                .send_key(KeyCode::Enter)
                .assert_empty();
        })
        .await;
        // Command is done, handle its resulting event
        component.int().drain_draw().assert_empty();

        // Make sure state updated correctly
        let data = component.data();
        assert_eq!(data.last_executed_query.as_deref(), Some("head -c 1"));
        assert_eq!(data.modified_text().as_deref(), Some("{"));
        assert_eq!(data.command_focus, CommandFocus::None);

        // Cancelling out of the text box should reset the query value
        component.int().send_key(KeyCode::Char('/')).assert_empty();
        component
            .int()
            .send_text("more text")
            .send_key(KeyCode::Esc)
            .assert_empty();
        let data = component.data();
        assert_eq!(data.last_executed_query.as_deref(), Some("head -c 1"));
        assert_eq!(data.query_text_box.data().text(), "head -c 1");
        assert_eq!(data.command_focus, CommandFocus::None);

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
        terminal: TestTerminal,
        response: Arc<ResponseRecord>,
    ) {
        // Add initial query to the DB
        DatabasePersistedStore::store_persisted(&Key, &"head -c 1".to_owned());

        // On init, we'll start executing the command in a local task. Wait for
        // that to finish
        let mut component = run_local(async {
            TestComponent::new(
                &harness,
                &terminal,
                PersistedLazy::new(
                    Key,
                    // Default value should get tossed out
                    QueryableBody::new(response, Some("initial".into())),
                ),
            )
        })
        .await;

        // After the command is done, there's a subsequent event with the result
        component.int().drain_draw().assert_empty();

        assert_eq!(
            component.data().last_executed_query.as_deref(),
            Some("head -c 1")
        );
        assert_eq!(&component.data().visible_text().to_string(), "{");
    }

    /// Test that the user's configured query default is applied on a fresh load
    #[rstest]
    #[tokio::test]
    async fn test_default_query_initial(
        harness: TestHarness,
        terminal: TestTerminal,
        response: Arc<ResponseRecord>,
    ) {
        // Local task is spawned to execute the initial subprocess
        let component = run_local(async {
            TestComponent::new(
                &harness,
                &terminal,
                QueryableBody::new(response, Some("head -n 1".into())),
            )
        })
        .await;

        assert_eq!(
            component.data().last_executed_query.as_deref(),
            Some("head -n 1")
        );
    }

    /// Test that the user's configured query default is applied when there's a
    /// persisted value, but it's an empty string
    #[rstest]
    #[tokio::test]
    async fn test_default_query_persisted(
        harness: TestHarness,
        terminal: TestTerminal,
        response: Arc<ResponseRecord>,
    ) {
        DatabasePersistedStore::store_persisted(&Key, &String::new());

        // Local task is spawned to execute the initial subprocess
        let component = run_local(async {
            TestComponent::new(
                &harness,
                &terminal,
                PersistedLazy::new(
                    Key,
                    // Default should override the persisted value
                    QueryableBody::new(response, Some("head -n 1".into())),
                ),
            )
        })
        .await;

        assert_eq!(
            component.data().last_executed_query.as_deref(),
            Some("head -n 1")
        );
    }

    /// Test an export command
    #[rstest]
    #[tokio::test]
    async fn test_export(
        harness: TestHarness,
        terminal: TestTerminal,
        response: Arc<ResponseRecord>,
        temp_dir: TempDir,
    ) {
        let mut component = TestComponent::builder(
            &harness,
            &terminal,
            QueryableBody::new(response, None),
        )
        .with_default_props()
        .with_area(terminal.area().inner(Margin {
            horizontal: 0,
            // Leave room for the text box scroll bar
            vertical: 1,
        }))
        .build();

        let path = temp_dir.join("test_export.json");
        let command = format!("tee {}", path.display());
        component
            .int()
            .send_key(KeyCode::Char(':'))
            .send_text(&command)
            .assert_empty();
        // Triggers the background task
        run_local(async {
            component.int().send_key(KeyCode::Enter).assert_empty();
        })
        .await;
        // Success should push a notification
        assert_matches!(
            component.int().drain_draw().events(),
            &[Event::Notify(_)]
        );
        let file_content = fs::read_to_string(&path).await.unwrap();
        assert_eq!(file_content, TEXT);

        // Error should appear as a modal
        component.int().send_text(":bad!").assert_empty();
        run_local(async {
            component.int().send_key(KeyCode::Enter).assert_empty();
        })
        .await;
        component.int().drain_draw().assert_empty();
        // Asserting on the modal within the view is a pain, so a shortcut is
        // to just make sure an error modal is present
        let modal = component.modal().expect("Error modal should be visible");
        assert_eq!(&modal.title().to_string(), "Error");
    }
}
