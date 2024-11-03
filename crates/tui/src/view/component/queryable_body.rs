//! Request/response body display component

use crate::{
    context::TuiContext,
    view::{
        common::{
            text_box::TextBox,
            text_window::{ScrollbarMargins, TextWindow, TextWindowProps},
        },
        context::UpdateContext,
        draw::{Draw, DrawMetadata},
        event::{Child, Event, EventHandler, Update},
        state::{Identified, StateCell},
        util::{highlight, str_to_text},
        Component, ViewContext,
    },
};
use anyhow::Context;
use persisted::PersistedContainer;
use ratatui::{
    layout::{Constraint, Layout},
    text::Text,
    Frame,
};
use serde_json_path::JsonPath;
use slumber_config::Action;
use slumber_core::{
    http::{content_type::ContentType, query::Query, ResponseBody},
    util::{MaybeStr, ResultTraced},
};
use std::cell::{Cell, Ref};

/// Display response body as text, with a query box to filter it if the body has
/// been parsed. The query state can be persisted by persisting this entire
/// container.
#[derive(Debug)]
pub struct QueryableBody {
    /// Visible text state. This needs to be in a cell because it's initialized
    /// from the body passed in via props
    state: StateCell<StateKey, State>,
    /// Store whether the body can be queried. True only if it's a recognized
    /// and parsed format
    query_available: Cell<bool>,
    /// Are we currently typing in the query box?
    query_focused: bool,
    /// Expression used to filter the content of the body down
    query: Option<Query>,
    /// Where the user enters their body query
    query_text_box: Component<TextBox>,
    /// Filtered text display
    text_window: Component<TextWindow>,
}

#[derive(Clone)]
pub struct QueryableBodyProps<'a> {
    /// Type of the body content; include for syntax highlighting
    pub content_type: Option<ContentType>,
    /// Body content. Theoretically this component isn't specific to responses,
    /// but that's the only place where it's used currently so we specifically
    /// accept a response body. By keeping it 90% agnostic (i.e. not accepting
    /// a full response), it makes it easier to adapt in the future if we want
    /// to make request bodies queryable as well.
    pub body: &'a ResponseBody,
}

#[derive(Clone, Debug, PartialEq)]
struct StateKey {
    /// Sometimes the parsing takes a little bit. We want to make sure the body
    /// is regenerated after parsing completes
    is_parsed: bool,
    query: Option<Query>,
}

#[derive(Debug)]
struct State {
    text: Identified<Text<'static>>,
    is_parsed: bool,
    is_binary: bool,
}

impl QueryableBody {
    /// Create a new body, optionally loading the query text from the
    /// persistence DB. This is optional because not all callers use the query
    /// box, or want to persist the value.
    pub fn new() -> Self {
        let input_engine = &TuiContext::get().input_engine;
        let binding = input_engine.binding_display(Action::Search);

        let text_box = TextBox::default()
            .placeholder(format!("'{binding}' to filter body with JSONPath"))
            .validator(|text| JsonPath::parse(text).is_ok())
            // Callback trigger an events, so we can modify our own state
            .on_click(|| {
                ViewContext::push_event(Event::new_local(QueryCallback::Focus))
            })
            .on_cancel(|| {
                ViewContext::push_event(Event::new_local(QueryCallback::Cancel))
            })
            .on_submit(|| {
                ViewContext::push_event(Event::new_local(QueryCallback::Submit))
            });
        Self {
            state: Default::default(),
            query_available: Cell::new(false),
            query_focused: false,
            query: Default::default(),
            query_text_box: text_box.into(),
            text_window: Default::default(),
        }
    }

    /// Get visible body text. Return an owned value because that's what all
    /// consumers need anyway, and it makes the API simpler. Return `None` if:
    /// - Text isn't initialized yet
    /// - Body is binary
    /// - Body has not been parsed, either because it's too large or not a known
    ///   content type
    ///
    /// Note that in the last case, we _could_ return the body, but it's going
    /// to be the same content as what's in the request store so we can avoid
    /// the clone by returning `None` instead.
    pub fn parsed_text(&self) -> Option<String> {
        let state = self.state.get()?;
        if state.is_binary || !state.is_parsed {
            None
        } else {
            Some(state.text.to_string())
        }
    }

    /// Get visible body text
    pub fn visible_text(&self) -> Option<Ref<'_, Text>> {
        self.state
            .get()
            .map(|state| Ref::map(state, |state| &*state.text))
    }
}

impl Default for QueryableBody {
    fn default() -> Self {
        Self::new()
    }
}

impl EventHandler for QueryableBody {
    fn update(&mut self, _: &mut UpdateContext, event: Event) -> Update {
        if let Some(Action::Search) = event.action() {
            if self.query_available.get() {
                self.query_focused = true;
            }
        } else if let Some(callback) = event.local::<QueryCallback>() {
            match callback {
                QueryCallback::Focus => self.query_focused = true,
                QueryCallback::Cancel => {
                    // Reset text to whatever was submitted last
                    self.query_text_box.data_mut().set_text(
                        self.query
                            .as_ref()
                            .map(Query::to_string)
                            .unwrap_or_default(),
                    );
                    self.query_focused = false;
                }
                QueryCallback::Submit => {
                    let text = self.query_text_box.data().text();
                    self.query = if text.is_empty() {
                        None
                    } else {
                        text.parse()
                            // Log the error, then throw it away
                            .with_context(|| {
                                format!("Error parsing query {text:?}")
                            })
                            .traced()
                            .ok()
                    };
                }
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

impl<'a> Draw<QueryableBodyProps<'a>> for QueryableBody {
    fn draw(
        &self,
        frame: &mut Frame,
        props: QueryableBodyProps,
        metadata: DrawMetadata,
    ) {
        // Body can only be queried if it's been parsed
        let query_available = props.body.parsed().is_some();
        self.query_available.set(query_available);

        let [body_area, query_area] = Layout::vertical([
            Constraint::Min(0),
            Constraint::Length(if query_available { 1 } else { 0 }),
        ])
        .areas(metadata.area());

        // Draw the body
        let body = props.body;
        let state_key = StateKey {
            query: self.query.clone(),
            is_parsed: props.body.parsed().is_some(),
        };
        let state = self.state.get_or_update(&state_key, || {
            init_state(props.content_type, body, self.query.as_ref())
        });
        self.text_window.draw(
            frame,
            TextWindowProps {
                text: &state.text,
                margins: ScrollbarMargins {
                    bottom: 2, // Extra margin to jump over the search box
                    ..Default::default()
                },
                footer: None,
            },
            body_area,
            true,
        );

        if query_available {
            self.query_text_box
                .draw(frame, (), query_area, self.query_focused);
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
        self.query_text_box.data_mut().restore_persisted(value)
    }
}

/// All callback events from the query text box
#[derive(Debug)]
enum QueryCallback {
    Focus,
    Cancel,
    Submit,
}

/// Calculate display text based on current body/query
fn init_state(
    content_type: Option<ContentType>,
    body: &ResponseBody,
    query: Option<&Query>,
) -> State {
    if TuiContext::get().config.http.is_large(body.size()) {
        // For bodies over the "large" size, skip prettification and
        // highlighting because it's slow. We could try to push this work
        // into a background thread instead, but there's way to kill those
        // threads so we could end up piling up a lot of work. It also burns
        // a lot of CPU, regardless of where it's run
        //
        // We don't show a hint to the user in this case because it's not
        // worth the screen real estate
        if let Some(text) = body.text() {
            State {
                text: str_to_text(text).into(),
                is_parsed: false,
                is_binary: false,
            }
        } else {
            // Showing binary content is a bit of a novelty, there's not much
            // value in it. For large bodies it's not worth the CPU cycles
            let text: Text = "<binary>".into();
            State {
                text: text.into(),
                is_parsed: false,
                is_binary: true,
            }
        }
    } else {
        // Query and prettify text if possible. This involves a lot of cloning
        // because it makes stuff easier. If it becomes a bottleneck on large
        // responses it's fixable.
        if let Some(parsed) = body.parsed() {
            // Body is a known content type so we parsed it - apply a query
            // if necessary and prettify the output
            let text = query
                .map(|query| query.query_content(parsed).prettify())
                .unwrap_or_else(|| parsed.prettify());
            let text = highlight::highlight_if(content_type, text.into());
            State {
                text: text.into(),
                is_parsed: true,
                is_binary: false,
            }
        } else if let Some(text) = body.text() {
            // Body is textual but hasn't been parsed. Just show the plain text
            State {
                text: str_to_text(text).into(),
                is_parsed: false,
                is_binary: false,
            }
        } else {
            // Content is binary, show a textual representation of it
            let text: Text = format!("{:#}", MaybeStr(body.bytes())).into();
            State {
                text: text.into(),
                is_parsed: false,
                is_binary: true,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        context::TuiContext,
        test_util::{
            harness, terminal, TestHarness, TestResponseParser, TestTerminal,
        },
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
    use slumber_core::{http::ResponseRecord, test_util::header_map};

    const TEXT: &[u8] = b"{\"greeting\":\"hello\"}";

    /// Style text to match the text window gutter
    fn gutter(text: &str) -> Span {
        let styles = &TuiContext::get().styles;
        Span::styled(text, styles.text_window.gutter)
    }

    #[fixture]
    fn json_response() -> ResponseRecord {
        let mut response = ResponseRecord {
            status: StatusCode::OK,
            headers: header_map([("Content-Type", "application/json")]),
            body: ResponseBody::new(TEXT.into()),
        };
        TestResponseParser::parse_body(&mut response);
        response
    }

    /// Render an unparsed body with no query box
    #[rstest]
    fn test_unparsed(
        harness: TestHarness,
        #[with(30, 2)] terminal: TestTerminal,
    ) {
        let body = ResponseBody::new(TEXT.into());
        let component = TestComponent::new(
            &harness,
            &terminal,
            QueryableBody::new(),
            QueryableBodyProps {
                content_type: None,
                body: &body,
            },
        );

        // Assert state
        let data = component.data();
        // Remove newline after this fix:
        // https://github.com/ratatui-org/ratatui/pull/1320
        let mut expected = String::from_utf8(TEXT.to_owned()).unwrap();
        expected.push('\n');
        assert_eq!(data.parsed_text().as_deref(), None);
        assert!(!data.query_available.get());
        assert_eq!(data.query, None);

        // Assert view
        terminal.assert_buffer_lines([
            vec![gutter("1"), " {\"greeting\":\"hello\"}    ".into()],
            vec![gutter(" "), "                             ".into()],
        ]);
    }

    /// Render a parsed body with query text box
    #[rstest]
    fn test_parsed(
        harness: TestHarness,
        #[with(32, 5)] terminal: TestTerminal,
        json_response: ResponseRecord,
    ) {
        let mut component = TestComponent::new(
            &harness,
            &terminal,
            QueryableBody::new(),
            QueryableBodyProps {
                content_type: None,
                body: &json_response.body,
            },
        );

        // Assert initial state/view
        let data = component.data();
        assert!(data.query_available.get());
        assert_eq!(data.query, None);
        assert_eq!(
            data.parsed_text().as_deref(),
            Some("{\n  \"greeting\": \"hello\"\n}")
        );
        let styles = &TuiContext::get().styles.text_box;
        terminal.assert_buffer_lines([
            vec![gutter("1"), " {                        ".into()],
            vec![gutter("2"), "   \"greeting\": \"hello\"".into()],
            vec![gutter("3"), " }                        ".into()],
            vec![gutter(" "), "                          ".into()],
            vec![Span::styled(
                "'/' to filter body with JSONPath",
                styles.text.patch(styles.placeholder),
            )],
        ]);

        // Type something into the query box
        component.send_key(KeyCode::Char('/')).assert_empty();
        component.send_text("$.greeting").assert_empty();
        component.send_key(KeyCode::Enter).assert_empty();

        // Make sure state updated correctly
        let data = component.data();
        assert_eq!(data.query, Some("$.greeting".parse().unwrap()));
        assert_eq!(data.parsed_text().as_deref(), Some("[\n  \"hello\"\n]"));
        assert!(data.query_focused); // Still focused

        // Cancelling out of the text box should reset the query value
        component.send_text("more text").assert_empty();
        component.send_key(KeyCode::Esc).assert_empty();
        let data = component.data();
        assert_eq!(data.query, Some("$.greeting".parse().unwrap()));
        assert_eq!(data.query_text_box.data().text(), "$.greeting");

        // Check the view again
        terminal.assert_buffer_lines([
            vec![gutter("1"), " [                        ".into()],
            vec![gutter("2"), "   \"hello\"              ".into()],
            vec![gutter("3"), " ]                        ".into()],
            vec![gutter(" "), "                          ".into()],
            vec![Span::styled(
                "$.greeting                      ",
                styles.text,
            )],
        ]);
    }

    /// Render a parsed body with query text box, and load initial query from
    /// the DB. This tests the `PersistedContainer` implementation
    #[rstest]
    fn test_persistence(
        harness: TestHarness,
        #[with(30, 4)] terminal: TestTerminal,
        json_response: ResponseRecord,
    ) {
        #[derive(Debug, Serialize, PersistedKey)]
        #[persisted(String)]
        struct Key;

        // Add initial query to the DB
        DatabasePersistedStore::store_persisted(&Key, &"$.greeting".to_owned());

        // We already have another test to check that querying works via typing
        // in the box, so we just need to make sure state is initialized
        // correctly here
        let component = TestComponent::new(
            &harness,
            &terminal,
            PersistedLazy::new(Key, QueryableBody::new()),
            QueryableBodyProps {
                content_type: None,
                body: &json_response.body,
            },
        );
        assert_eq!(component.data().query, Some("$.greeting".parse().unwrap()));
    }
}
