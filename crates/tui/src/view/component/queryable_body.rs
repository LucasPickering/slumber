use crate::{
    message::Message,
    util,
    view::{
        Component, Generate, ViewContext,
        common::{
            text_box::{TextBox, TextBoxProps},
            text_window::{ScrollbarMargins, TextWindow, TextWindowProps},
        },
        component::{
            Canvas, Child, ComponentExt, ComponentId, Draw, DrawMetadata,
            ToChild,
            command_text_box::{CommandTextBox, CommandTextBoxEvent},
        },
        context::UpdateContext,
        event::{Emitter, Event, EventMatch, ToEmitter},
        persistent::{PersistentKey, PersistentStore},
        util::{highlight, str_to_text},
    },
};
use anyhow::Context;
use bytes::Bytes;
use ratatui::{
    layout::{Constraint, Layout},
    text::Text,
};
use slumber_config::Action;
use slumber_core::{
    http::{ResponseBody, ResponseRecord, content_type::ContentType},
    util::MaybeStr,
};
use std::{borrow::Cow, mem, sync::Arc};
use tokio_util::sync::CancellationToken;

/// Display response body as text, with a query box to run commands on the body.
/// The query state can be persisted by persisting this entire container.
#[derive(Debug)]
pub struct QueryableBody<K> {
    id: ComponentId,
    emitter: Emitter<CommandComplete>,
    response: Arc<ResponseRecord>,
    persistent_key: K,

    /// Which command box, if any, are we typing in?
    command_focus: CommandFocus,
    /// Track status of the current query command
    query_state: CommandState,
    /// Where the user enters their body query
    query_text_box: CommandTextBox,
    /// Query command to reset back to when the user hits cancel
    last_executed_query: Option<String>,

    /// Export command, for side effects. This isn't persistent, so the state
    /// is a lot simpler. We'll clear this out whenever the user exits.
    export_text_box: CommandTextBox,

    /// Filtered text display
    text_state: TextState,
}

impl<K> QueryableBody<K> {
    /// Create a new body with an optional default query
    pub fn new(
        persistent_key: K,
        response: Arc<ResponseRecord>,
        default_query: Option<String>,
    ) -> Self
    where
        K: PersistentKey<Value = String>,
    {
        let query_bind = ViewContext::binding_display(Action::Search);
        let export_bind = ViewContext::binding_display(Action::Export);

        // Load query from the store. Fall back to the default if missing
        let query = PersistentStore::get(&persistent_key)
            // It's pretty common to clear the whole text box without thinking
            // about it. In that case, we want to restore the default the next
            // time we reload from persistence (probably either app restart or
            // next response for this recipe). It's possible the user really
            // wants an empty box and this is annoying, but I think it'll be
            // more good than bad.
            .filter(|query| !query.is_empty())
            .or(default_query);

        let query_text_box = CommandTextBox::new(
            TextBox::default()
                .placeholder(format!(
                    "{query_bind} to query, {export_bind} to export"
                ))
                .placeholder_focused("Enter query command (ex: `jq .results`)")
                .default_value(query.unwrap_or_default()),
        );
        let export_text_box =
            CommandTextBox::new(TextBox::default().placeholder_focused(
                "Enter export command (ex: `tee > response.json`)",
            ));

        let text_state =
            TextState::new(response.content_type(), &response.body, true);

        let mut slf = Self {
            id: ComponentId::default(),
            emitter: Default::default(),
            response,
            persistent_key,
            command_focus: CommandFocus::None,
            query_state: CommandState::None,
            query_text_box,
            last_executed_query: None,
            export_text_box,
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
        if matches!(self.query_state, CommandState::Ok)
            || self.text_state.pretty
        {
            Some(self.text_state.text_window.text().to_string())
        } else {
            None
        }
    }

    /// Get whatever text the user sees
    pub fn visible_text(&self) -> &Text<'_> {
        self.text_state.text_window.text()
    }

    fn focus(&mut self, focus: CommandFocus) {
        self.command_focus = focus;
    }

    /// Update query command based on the current text in the box, and start
    /// a task to run the command
    fn update_query(&mut self) {
        let command = self.query_text_box.text().trim();

        // If the command hasn't changed, do nothing
        if self.last_executed_query.as_deref() == Some(command) {
            return;
        }

        // If a different command is already running, abort it
        if let Some(token) = self.query_state.take_cancel_token() {
            token.cancel();
        }

        if command.is_empty() {
            // Reset to initial body
            self.last_executed_query = None;
            self.query_state = CommandState::None;
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
            let cancel_token =
                self.spawn_command(command, body, move |_, result| {
                    emitter.emit(CommandComplete(result));
                });
            self.query_state = CommandState::Running(cancel_token);
        }
    }

    /// Run an export shell command with the response as stdin. The output
    /// will *not* be reflected in the UI. Used for things like saving a
    /// response to a file.
    fn export(&mut self) {
        let command = self.export_text_box.clear();

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
            // We provide feedback via a global mechanism in both cases, so
            // we don't need an emitter here
            Ok(_) => ViewContext::send_message(Message::Notify(format!(
                "`{command}` succeeded"
            ))),
            Err(error) => ViewContext::send_message(Message::Error { error }),
        });
    }

    /// Run the current text as a shell command in a background task
    ///
    /// Return a cancellation token that can be used to cancel the process
    fn spawn_command(
        &self,
        command: String,
        body: Bytes,
        on_complete: impl 'static + FnOnce(String, anyhow::Result<Vec<u8>>),
    ) -> CancellationToken {
        let cancel_token = CancellationToken::new();
        let future = async move {
            // Store the command in history. Query and export commands are
            // stored together. We can toss the error; it gets traced by the DB
            let _ =
                ViewContext::with_database(|db| db.insert_command(&command));
            let shell = &ViewContext::config().tui.commands.shell;
            let result = util::run_command(shell, &command, Some(&body))
                .await
                .with_context(|| format!("Error running `{command}`"));
            on_complete(command, result);
        };
        ViewContext::spawn(util::cancellable(&cancel_token, future));
        cancel_token
    }
}

impl<K: PersistentKey<Value = String>> Component for QueryableBody<K> {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn update(
        &mut self,
        context: &mut UpdateContext,
        event: Event,
    ) -> EventMatch {
        event
            .m()
            .click(|position, _| {
                // Focus the query text box when clicked. No need to check for
                // clicks on the export box since it isn't drawn unless focused
                if self.query_text_box.contains(context, position) {
                    self.focus(CommandFocus::Query);
                }
            })
            .action(|action, propagate| match action {
                Action::Search => self.focus(CommandFocus::Query),
                Action::Export => self.focus(CommandFocus::Export),
                _ => propagate.set(),
            })
            .emitted(self.emitter, |CommandComplete(result)| match result {
                Ok(stdout) => {
                    self.query_state = CommandState::Ok;
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
                Err(error) => self.query_state = CommandState::Error(error),
            })
            .emitted(self.query_text_box.to_emitter(), |event| match event {
                CommandTextBoxEvent::Cancel => {
                    // Reset text to whatever was submitted last
                    self.query_text_box.set_text(
                        self.last_executed_query.clone().unwrap_or_default(),
                    );
                    self.focus(CommandFocus::None);
                }
                CommandTextBoxEvent::Submit => {
                    self.update_query();
                    self.focus(CommandFocus::None);
                }
            })
            .emitted(self.export_text_box.to_emitter(), |event| match event {
                CommandTextBoxEvent::Cancel => {
                    self.export_text_box.clear();
                    self.focus(CommandFocus::None);
                }
                CommandTextBoxEvent::Submit => {
                    self.export();
                    self.focus(CommandFocus::None);
                }
            })
    }

    fn persist(&self, store: &mut PersistentStore) {
        store.set(&self.persistent_key, &self.query_text_box.text().to_owned());
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        vec![
            self.query_text_box.to_child_mut(),
            self.export_text_box.to_child_mut(),
            self.text_state.text_window.to_child_mut(),
        ]
    }
}

impl<K: PersistentKey<Value = String>> Draw for QueryableBody<K> {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        let [body_area, query_area] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(1)])
                .areas(metadata.area());

        if let CommandState::Error(error) = &self.query_state {
            canvas.render_widget(error.generate(), body_area);
        } else {
            canvas.draw(
                &self.text_state.text_window,
                TextWindowProps {
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
            canvas.draw(
                &self.export_text_box,
                TextBoxProps::default(),
                query_area,
                true,
            );
        } else {
            canvas.draw(
                &self.query_text_box,
                TextBoxProps {
                    has_error: matches!(
                        self.query_state,
                        CommandState::Error(_)
                    ),
                    ..TextBoxProps::default()
                },
                query_area,
                self.command_focus == CommandFocus::Query,
            );
        }
    }
}

impl<K> ToEmitter<CommandComplete> for QueryableBody<K> {
    fn to_emitter(&self) -> Emitter<CommandComplete> {
        self.emitter
    }
}

/// Rendered body text. This encapsulates everything that can change when the
/// body or command changes.
#[derive(Debug)]
struct TextState {
    /// Visible text
    text_window: TextWindow,
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
        if ViewContext::config().http.is_large(body.size()) {
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
                    text_window: TextWindow::new(str_to_text(text)),
                    pretty: false,
                }
            } else {
                // Showing binary content is a bit of a novelty, there's not
                // much value in it. For large bodies it's not worth the CPU
                // cycles
                let text: Text = "<binary>".into();
                TextState {
                    text_window: TextWindow::new(text),
                    pretty: false,
                }
            }
        } else if let Some(text) = body.text() {
            // Prettify for known content types. We _don't_ do this in a
            // separate task because it's generally very fast. If this is slow
            // enough that it affects the user, the "large" body size is
            // probably too low
            let (text, pretty): (Cow<str>, bool) = if let Some(content_type) =
                content_type
                && prettify
            {
                content_type
                    .prettify(text)
                    .map(|body| (Cow::Owned(body), true))
                    .unwrap_or((Cow::Borrowed(text), false))
            } else {
                (Cow::Borrowed(text), false)
            };

            let text =
                highlight::highlight_if(content_type, str_to_text(&text));
            TextState {
                text_window: TextWindow::new(text),
                pretty,
            }
        } else {
            // Content is binary, show a textual representation of it
            let text: Text =
                format!("{:#}", MaybeStr(body.bytes().as_ref())).into();
            TextState {
                text_window: TextWindow::new(text),
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
struct CommandComplete(Result<Vec<u8>, anyhow::Error>);

#[derive(Debug, Default)]
enum CommandState {
    /// Command has not been run yet
    #[default]
    None,
    /// Command is running. Token can be used to kill it
    Running(CancellationToken),
    /// Command failed
    Error(anyhow::Error),
    // Success! The result is immediately transformed and stored in the text
    // window, so we don't need to store it here
    Ok,
}

impl CommandState {
    fn take_cancel_token(&mut self) -> Option<CancellationToken> {
        match mem::take(self) {
            Self::Running(token) => Some(token),
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
        test_util::{TestTerminal, terminal},
        view::{
            context::ViewContext,
            test_util::{TestComponent, TestHarness, harness},
        },
    };
    use ratatui::{layout::Margin, text::Span};
    use rstest::{fixture, rstest};
    use serde::Serialize;
    use slumber_core::http::{ResponseBody, ResponseRecord};
    use slumber_util::{Factory, TempDir, assert_matches, temp_dir};
    use terminput::KeyCode;
    use tokio::fs;

    const TEXT: &str = "{\"greeting\":\"hello\"}";

    /// Persistence key for testing
    #[derive(Debug, Serialize)]
    struct Key;

    impl PersistentKey for Key {
        type Value = String;
    }

    /// Style text to match the text window gutter
    fn gutter(text: &str) -> Span<'_> {
        let styles = ViewContext::styles();
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
        mut harness: TestHarness,
        #[with(27, 3)] terminal: TestTerminal,
        response: Arc<ResponseRecord>,
    ) {
        let mut component = TestComponent::new(
            &harness,
            &terminal,
            QueryableBody::new(Key, response, None),
        );

        // Assert initial state/view
        assert_eq!(component.last_executed_query, None);
        assert_eq!(component.modified_text().as_deref(), None);
        let styles = ViewContext::styles().text_box;
        terminal.assert_buffer_lines([
            vec![gutter("1"), " {\"greeting\":\"hello\"}".into()],
            vec![gutter(" "), "".into()],
            vec![Span::styled(
                "[/] to query, [:] to export",
                styles.text.patch(styles.placeholder),
            )],
        ]);

        // Type something into the query box
        component
            .int()
            .send_key(KeyCode::Char('/'))
            .send_text("head -c 1")
            .send_key(KeyCode::Enter)
            .assert()
            .empty();
        harness.run_task().await; // Run spawned command
        // Command is done, handle its resulting event
        component.int().drain_draw().assert().empty();

        // Make sure state updated correctly
        assert_eq!(component.last_executed_query.as_deref(), Some("head -c 1"));
        assert_eq!(component.modified_text().as_deref(), Some("{"));
        assert_eq!(component.command_focus, CommandFocus::None);

        // Cancelling out of the text box should reset the query value
        component
            .int()
            .send_key(KeyCode::Char('/'))
            .assert()
            .empty();
        component
            .int()
            .send_text("more text")
            .send_key(KeyCode::Esc)
            .assert()
            .empty();
        assert_eq!(component.last_executed_query.as_deref(), Some("head -c 1"));
        assert_eq!(component.query_text_box.text(), "head -c 1");
        assert_eq!(component.command_focus, CommandFocus::None);

        // Check the view again
        terminal.assert_buffer_lines([
            vec![gutter("1"), " {                   ".into()],
            vec![gutter(" "), "                     ".into()],
            vec![Span::styled("head -c 1                  ", styles.text)],
        ]);
    }

    /// Render a parsed body with query text box, and load initial query from
    /// the DB. This tests the persistence implementation
    #[rstest]
    #[tokio::test]
    async fn test_persistence(
        mut harness: TestHarness,
        terminal: TestTerminal,
        response: Arc<ResponseRecord>,
    ) {
        // Add initial query to the DB
        harness
            .persistent_store()
            .set(&Key, &"head -c 1".to_owned());

        let mut component = TestComponent::new(
            &harness,
            &terminal,
            // Default value should get tossed out
            QueryableBody::new(Key, response, Some("initial".into())),
        );
        harness.run_task().await; // Run the initial task

        // After the command is done, there's a subsequent event with the result
        component.int().drain_draw().assert().empty();

        assert_eq!(component.last_executed_query.as_deref(), Some("head -c 1"));
        assert_eq!(&component.visible_text().to_string(), "{");
    }

    /// Test that the user's configured query default is applied on a fresh load
    #[rstest]
    #[tokio::test]
    async fn test_default_query_initial(
        mut harness: TestHarness,
        terminal: TestTerminal,
        response: Arc<ResponseRecord>,
    ) {
        let component = TestComponent::new(
            &harness,
            &terminal,
            QueryableBody::new(Key, response, Some("head -n 1".into())),
        );
        harness.run_task().await; // Run the initial task

        assert_eq!(component.last_executed_query.as_deref(), Some("head -n 1"));
    }

    /// Test that the user's configured query default is applied when there's a
    /// persisted value, but it's an empty string
    #[rstest]
    #[tokio::test]
    async fn test_default_query_persisted(
        mut harness: TestHarness,
        terminal: TestTerminal,
        response: Arc<ResponseRecord>,
    ) {
        harness.persistent_store().set(&Key, &String::new());

        let component = TestComponent::new(
            &harness,
            &terminal,
            // Default should override the persisted value
            QueryableBody::new(Key, response, Some("head -n 1".into())),
        );
        harness.run_task().await; // Run the initial task

        assert_eq!(component.last_executed_query.as_deref(), Some("head -n 1"));
    }

    /// Test an export command
    #[rstest]
    #[tokio::test]
    async fn test_export(
        mut harness: TestHarness,
        terminal: TestTerminal,
        response: Arc<ResponseRecord>,
        temp_dir: TempDir,
    ) {
        let mut component = TestComponent::builder(
            &harness,
            &terminal,
            QueryableBody::new(Key, response, None),
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
            .assert()
            .empty();

        // Trigger the background task, then run it
        component.int().send_key(KeyCode::Enter).assert().empty();
        harness.run_task().await;

        // Success should push a notification
        assert_matches!(harness.messages().pop_now(), Message::Notify(_));
        let file_content = fs::read_to_string(&path).await.unwrap();
        assert_eq!(file_content, TEXT);

        // Error should be sent as a message. Testing that the error is actually
        // displayed is someone else's problem!!
        component
            .int()
            .send_key(KeyCode::Char(':'))
            .send_text("bad!")
            .assert()
            .empty();
        component.int().send_key(KeyCode::Enter).assert().empty();
        harness.run_task().await;
        component.int().drain_draw().assert().empty();
        assert_matches!(harness.messages().pop_now(), Message::Error { .. });
    }
}
