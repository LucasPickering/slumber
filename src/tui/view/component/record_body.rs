//! Request/response body display component

use crate::{
    http::{Body, Query},
    tui::{
        input::Action,
        view::{
            common::{
                text_box::TextBox,
                text_window::{TextWindow, TextWindowProps},
            },
            draw::{Draw, DrawMetadata},
            event::{Event, EventHandler, Update},
            state::{
                persistence::{Persistent, PersistentKey},
                StateCell,
            },
            Component, ViewContext,
        },
    },
    util::{MaybeStr, ResultExt},
};
use anyhow::Context;
use ratatui::{
    layout::{Constraint, Layout},
    Frame,
};
use serde_json_path::JsonPath;
use std::cell::Cell;
use Debug;

/// Display text body of a request/response
#[derive(Debug)]
pub struct RecordBody {
    /// Body text content. State cell allows us to reset this whenever the
    /// request changes
    text_window: StateCell<Option<Query>, Component<TextWindow<String>>>,
    /// Store whether the body can be queried. True only if it's a recognized
    /// and parsed format
    query_available: Cell<bool>,
    /// Expression used to filter the content of the body down
    query: Option<Query>,
    /// Where the user enters their body query
    query_text_box: Component<Persistent<TextBox>>,
}

pub struct RecordBodyProps<'a> {
    pub body: &'a Body,
}

/// Callback event from the query text box when user hits Enter
struct QuerySubmit(String);

impl RecordBody {
    /// Create a new body, optionally loading the query text from the
    /// persistence DB. This is optional because not all callers use the query
    /// box, or want to persist the value.
    pub fn new(query_persistent_key: Option<PersistentKey>) -> Self {
        let text_box = TextBox::default()
            .with_focus(false)
            .with_placeholder("'/' to filter body with JSONPath")
            .with_validator(|text| JsonPath::parse(text).is_ok())
            // Callback triggers an event, so we can modify our own state
            .with_on_submit(|text_box| {
                ViewContext::push_event(Event::new_other(QuerySubmit(
                    text_box.text().to_owned(),
                )))
            });
        Self {
            text_window: Default::default(),
            query_available: Cell::new(false),
            query: Default::default(),
            query_text_box: Persistent::optional(
                query_persistent_key,
                text_box,
            )
            .into(),
        }
    }

    /// Get visible body text
    pub fn text(&self) -> Option<String> {
        self.text_window
            .get()
            .map(|text_window| text_window.data().text().to_owned())
    }
}

impl EventHandler for RecordBody {
    fn update(&mut self, event: Event) -> Update {
        match event {
            Event::Input {
                action: Some(Action::Search),
                ..
            } if self.query_available.get() => {
                self.query_text_box.data_mut().focus()
            }
            Event::Other(ref other) => {
                match other.downcast_ref::<QuerySubmit>() {
                    Some(QuerySubmit(text)) => {
                        self.query = text
                            .parse()
                            // Log the error, then throw it away
                            .with_context(|| {
                                format!("Error parsing query {text:?}")
                            })
                            .traced()
                            .ok();
                    }
                    None => return Update::Propagate(event),
                }
            }
            _ => return Update::Propagate(event),
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

impl<'a> Draw<RecordBodyProps<'a>> for RecordBody {
    fn draw(
        &self,
        frame: &mut Frame,
        props: RecordBodyProps,
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
            self.query_text_box.draw(
                frame,
                (),
                query_area,
                self.query_text_box.data().has_focus(),
            );
        }
    }
}

fn init_text_window(
    body: &Body,
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
        collection::RecipeId, db::CollectionDatabase, http::Response,
        test_util::*, tui::context::TuiContext,
    };
    use crossterm::event::{
        KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers,
    };
    use ratatui::{backend::TestBackend, buffer::Buffer, text::Span, Terminal};
    use reqwest::StatusCode;
    use rstest::{fixture, rstest};

    const TEXT: &[u8] = b"{\"greeting\":\"hello\"}";

    /// Draw a component onto the terminal. Prefer this over calling
    /// [Component::draw] directly, because that won't update Ratatui's internal
    /// buffers correctly.
    /// TODO move somewhere else
    fn draw<Props, T: Draw<Props>>(
        terminal: &mut Terminal<TestBackend>,
        component: &Component<T>,
        props: Props,
    ) {
        terminal
            .draw(|frame| component.draw(frame, props, frame.size(), true))
            .unwrap();
    }

    /// Generate an event for a keypress, and push it onto the event queue
    /// TODO move
    fn send_key(
        code: KeyCode,
        modifiers: KeyModifiers,
        action: Option<Action>,
    ) {
        let crossterm_event = crossterm::event::Event::Key(KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        });
        let event = Event::Input {
            event: crossterm_event,
            action,
        };
        ViewContext::push_event(event);
    }

    /// Send some text as a series of key events
    /// TODO move
    fn send_text(text: &str) {
        for c in text.chars() {
            send_key(KeyCode::Char(c), KeyModifiers::NONE, None);
        }
    }

    /// Style text to match the text window gutter
    fn gutter(text: &str) -> Span {
        let styles = &TuiContext::get().styles;
        Span::styled(text, styles.text_window.gutter)
    }

    #[fixture]
    fn json_response() -> Response {
        let response = Response {
            status: StatusCode::OK,
            headers: header_map([("Content-Type", "application/json")]),
            body: Body::new(TEXT.into()),
        };
        response.parse_body();
        response
    }

    /// Render an unparsed body with no query box
    #[rstest]
    fn test_unparsed(
        _tui_context: &TuiContext,
        database: CollectionDatabase,
        messages: MessageQueue,
        #[with(30, 2)] mut terminal: Terminal<TestBackend>,
    ) {
        ViewContext::init(database.clone(), messages.tx().clone());
        let component = RecordBody::new(None).into();
        let body = Body::new(TEXT.into());
        draw(&mut terminal, &component, RecordBodyProps { body: &body });

        // Assert state
        let data = component.data();
        assert_eq!(
            data.text().as_deref(),
            Some(std::str::from_utf8(TEXT).unwrap())
        );
        assert!(!data.query_available.get());
        assert_eq!(data.query, None);

        // Assert view
        // TODO use assert_buffer_lines
        terminal.backend().assert_buffer(&Buffer::with_lines([
            vec![gutter("1"), " {\"greeting\":\"hello\"}    ".into()],
            vec![gutter(" "), "                             ".into()],
        ]));
    }

    /// Render a parsed body with query text box
    #[rstest]
    fn test_parsed(
        tui_context: &TuiContext,
        database: CollectionDatabase,
        messages: MessageQueue,
        json_response: Response,
        #[with(32, 5)] mut terminal: Terminal<TestBackend>,
    ) {
        ViewContext::init(database.clone(), messages.tx().clone());
        let mut component = RecordBody::new(None).into();
        let props = || RecordBodyProps {
            body: &json_response.body,
        };
        draw(&mut terminal, &component, props());

        // Assert state
        let data = component.data();
        assert!(data.query_available.get());
        assert_eq!(data.query, None);
        assert_eq!(
            data.text().as_deref(),
            Some("{\n  \"greeting\": \"hello\"\n}")
        );

        // Assert view
        let styles = &tui_context.styles.text_box;
        terminal.backend().assert_buffer(&Buffer::with_lines([
            vec![gutter("1"), " {                        ".into()],
            vec![gutter("2"), "   \"greeting\": \"hello\"".into()],
            vec![gutter("3"), " }                        ".into()],
            vec![gutter(" "), "                          ".into()],
            vec![Span::styled(
                "'/' to filter body with JSONPath",
                styles.text.patch(styles.placeholder),
            )],
        ]));

        // Type something into the query box
        send_key(KeyCode::Char('/'), KeyModifiers::NONE, Some(Action::Search));
        component.drain_events();
        // Re-draw to update focus for text box
        draw(&mut terminal, &component, props());
        send_text("$.greeting");
        send_key(KeyCode::Enter, KeyModifiers::NONE, Some(Action::Submit));
        component.drain_events();
        // Re-draw again to apply query to body
        draw(&mut terminal, &component, props());

        // Make sure state updated correctly
        let data = component.data();
        assert_eq!(data.query, Some("$.greeting".parse().unwrap()));
        assert_eq!(data.text().as_deref(), Some("[\n  \"hello\"\n]"));

        // Check the view again too
        terminal.backend().assert_buffer(&Buffer::with_lines([
            vec![gutter("1"), " [                        ".into()],
            vec![gutter("2"), "   \"hello\"              ".into()],
            vec![gutter("3"), " ]                        ".into()],
            vec![gutter(" "), "                          ".into()],
            vec![Span::styled(
                "$.greeting                      ",
                styles.text,
            )],
        ]));
    }

    /// Render a parsed body with query text box, and initial query from the DB
    #[rstest]
    fn test_initial_query(
        _tui_context: &TuiContext,
        database: CollectionDatabase,
        messages: MessageQueue,
        json_response: Response,
        #[with(30, 4)] mut terminal: Terminal<TestBackend>,
    ) {
        ViewContext::init(database.clone(), messages.tx().clone());
        let recipe_id = RecipeId::factory(());

        // Add initial query to the DB
        let persistent_key =
            PersistentKey::ResponseBodyQuery(recipe_id.clone());
        database.set_ui(&persistent_key, "$.greeting").unwrap();

        // We already have another test to check that querying works via typing
        // in the box, so we just need to make sure state is initialized
        // correctly here
        let mut component: Component<_> =
            RecordBody::new(Some(persistent_key)).into();
        draw(
            &mut terminal,
            &component,
            RecordBodyProps {
                body: &json_response.body,
            },
        );
        component.drain_events(); // Events are triggered during init
        assert_eq!(component.data().query, Some("$.greeting".parse().unwrap()));
    }
}
