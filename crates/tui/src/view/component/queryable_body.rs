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
        state::StateCell,
        util::highlight,
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
use std::cell::Cell;

/// Display response body as text, with a query box to filter it if the body has
/// been parsed. The query state can be persisted by persisting this entire
/// container.
#[derive(Debug)]
pub struct QueryableBody {
    /// Visible text state. This needs to be in a cell because it's initialized
    /// from the body passed in via props
    filtered_text: StateCell<Option<Query>, Text<'static>>,
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
            filtered_text: Default::default(),
            query_available: Cell::new(false),
            query_focused: false,
            query: Default::default(),
            query_text_box: text_box.into(),
            text_window: Default::default(),
        }
    }

    /// Get visible body text. Return an owned value because that's what all
    /// consumers need anyway, and it makes the API simpler
    pub fn text(&self) -> Option<String> {
        self.filtered_text.get().map(|text| text.to_string())
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
        let text = self.filtered_text.get_or_update(&self.query, || {
            init_text(props.content_type, props.body, self.query.as_ref())
        });
        self.text_window.draw(
            frame,
            TextWindowProps {
                text: &text,
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
fn init_text(
    content_type: Option<ContentType>,
    body: &ResponseBody,
    query: Option<&Query>,
) -> Text<'static> {
    // Query and prettify text if possible. This involves a lot of cloning
    // because it makes stuff easier. If it becomes a bottleneck on large
    // responses it's fixable.
    let body = body
        .parsed()
        .map(|parsed_body| {
            // Body is a known content type so we parsed it - apply a query if
            // necessary and prettify the output
            query
                .map(|query| query.query_content(parsed_body).prettify())
                .unwrap_or_else(|| parsed_body.prettify())
        })
        // Content couldn't be parsed, fall back to the raw text
        // If the text isn't UTF-8, we'll show a placeholder instead
        .unwrap_or_else(|| format!("{:#}", MaybeStr(body.bytes())));
    // Apply syntax highlighting
    highlight::highlight_if(content_type, body.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        context::TuiContext,
        test_util::{harness, terminal, TestHarness, TestTerminal},
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
        let response = ResponseRecord {
            status: StatusCode::OK,
            headers: header_map([("Content-Type", "application/json")]),
            body: ResponseBody::new(TEXT.into()),
        };
        response.parse_body();
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
        assert_eq!(data.text().as_deref(), Some(expected.as_str()));
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
            data.text().as_deref(),
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
        assert_eq!(data.text().as_deref(), Some("[\n  \"hello\"\n]"));
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
