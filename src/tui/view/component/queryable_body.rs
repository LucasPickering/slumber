//! Request/response body display component

use crate::{
    http::{Query, ResponseBody},
    tui::{
        input::Action,
        view::{
            common::{
                text_box::TextBox,
                text_window::{TextWindow, TextWindowProps},
            },
            draw::{Draw, DrawMetadata},
            event::{Event, EventHandler, Update},
            state::StateCell,
            Component, ViewContext,
        },
    },
    util::{MaybeStr, ResultExt},
};
use anyhow::Context;
use persisted::PersistedContainer;
use ratatui::{
    layout::{Constraint, Layout},
    Frame,
};
use serde_json_path::JsonPath;
use std::cell::Cell;
use Debug;

/// Display response body as text, with a query box to filter it if the body has
/// been parsed. The query state can be persisted by persisting this entire
/// container.
#[derive(Debug)]
pub struct QueryableBody {
    /// Body text content. State cell allows us to reset this whenever the
    /// request changes
    text_window: StateCell<Option<Query>, Component<TextWindow<String>>>,
    /// Store whether the body can be queried. True only if it's a recognized
    /// and parsed format
    query_available: Cell<bool>,
    /// Are we currently typing in the query box?
    query_focused: bool,
    /// Expression used to filter the content of the body down
    query: Option<Query>,
    /// Where the user enters their body query
    query_text_box: Component<TextBox>,
}

#[derive(Clone)]
pub struct QueryableBodyProps<'a> {
    pub body: &'a ResponseBody,
}

impl QueryableBody {
    /// Create a new body, optionally loading the query text from the
    /// persistence DB. This is optional because not all callers use the query
    /// box, or want to persist the value.
    pub fn new() -> Self {
        let text_box = TextBox::default()
            .placeholder("'/' to filter body with JSONPath")
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
            text_window: Default::default(),
            query_available: Cell::new(false),
            query_focused: false,
            query: Default::default(),
            query_text_box: text_box.into(),
        }
    }

    /// Get visible body text
    pub fn text(&self) -> Option<String> {
        self.text_window
            .get()
            .map(|text_window| text_window.data().text().to_owned())
    }
}

impl Default for QueryableBody {
    fn default() -> Self {
        Self::new()
    }
}

impl EventHandler for QueryableBody {
    fn update(&mut self, event: Event) -> Update {
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

    fn children(&mut self) -> Vec<Component<&mut dyn EventHandler>> {
        [
            Some(self.query_text_box.as_child()),
            self.text_window.get_mut().map(Component::as_child),
        ]
        .into_iter()
        .flatten()
        .collect()
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
        let text = self.text_window.get_or_update(self.query.clone(), || {
            init_text_window(props.body, self.query.as_ref())
        });
        text.draw(
            frame,
            TextWindowProps {
                has_search_box: query_available,
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

    fn get_persisted(&self) -> Self::Value {
        self.query_text_box.data().get_persisted()
    }

    fn set_persisted(&mut self, value: Self::Value) {
        self.query_text_box.data_mut().set_persisted(value)
    }
}

/// All callback events from the query text box
#[derive(Debug)]
enum QueryCallback {
    Focus,
    Cancel,
    Submit,
}

fn init_text_window(
    body: &ResponseBody,
    query: Option<&Query>,
) -> Component<TextWindow<String>> {
    // Query and prettify text if possible. This involves a lot of cloning
    // because it makes stuff easier. If it becomes a bottleneck on large
    // responses it's fixable.
    let body = body
        .parsed()
        .map(|parsed_body| {
            // Body is a known content type so we parsed it - apply a query if
            // necessary and prettify the output
            query
                .map(|query| query.query(parsed_body).prettify())
                .unwrap_or_else(|| parsed_body.prettify())
        })
        // Content couldn't be parsed, fall back to the raw text
        // If the text isn't UTF-8, we'll show a placeholder instead
        .unwrap_or_else(|| format!("{:#}", MaybeStr(body.bytes())));

    TextWindow::new(body).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        http::ResponseRecord,
        test_util::header_map,
        tui::{
            context::TuiContext,
            test_util::{harness, TestHarness},
            view::{context::PersistedLazy, test_util::TestComponent},
        },
    };
    use crossterm::event::KeyCode;
    use persisted::{PersistedKey, PersistedStore};
    use ratatui::text::Span;
    use reqwest::StatusCode;
    use rstest::{fixture, rstest};
    use serde::Serialize;

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
    fn test_unparsed(#[with(30, 2)] harness: TestHarness) {
        let body = ResponseBody::new(TEXT.into());
        let component = TestComponent::new(
            harness,
            QueryableBody::new(),
            QueryableBodyProps { body: &body },
        );

        // Assert state
        let data = component.data();
        assert_eq!(
            data.text().as_deref(),
            Some(std::str::from_utf8(TEXT).unwrap())
        );
        assert!(!data.query_available.get());
        assert_eq!(data.query, None);

        // Assert view
        component.assert_buffer_lines([
            vec![gutter("1"), " {\"greeting\":\"hello\"}    ".into()],
            vec![gutter(" "), "                             ".into()],
        ]);
    }

    /// Render a parsed body with query text box
    #[rstest]
    fn test_parsed(
        #[with(32, 5)] harness: TestHarness,
        json_response: ResponseRecord,
    ) {
        let mut component = TestComponent::new(
            harness,
            QueryableBody::new(),
            QueryableBodyProps {
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
        component.assert_buffer_lines([
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
        component.assert_buffer_lines([
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
        #[with(30, 4)] harness: TestHarness,
        json_response: ResponseRecord,
    ) {
        #[derive(Debug, Serialize, PersistedKey)]
        #[persisted(String)]
        struct Key;

        // Add initial query to the DB
        ViewContext::store_persisted(&Key, "$.greeting".to_owned());

        // We already have another test to check that querying works via typing
        // in the box, so we just need to make sure state is initialized
        // correctly here
        let component = TestComponent::new(
            harness,
            PersistedLazy::new(Key, QueryableBody::new()),
            QueryableBodyProps {
                body: &json_response.body,
            },
        );
        assert_eq!(component.data().query, Some("$.greeting".parse().unwrap()));
    }
}
