//! Response body display component

use crate::{
    http::{Query, RequestId, RequestRecord, ResponseContent},
    tui::{
        input::Action,
        view::{
            common::{text_box::TextBox, text_window::TextWindow},
            draw::Draw,
            event::{Event, EventHandler, Update, UpdateContext},
            state::StateCell,
            util::layout,
            Component,
        },
    },
    util::{MaybeStr, ResultExt},
};
use anyhow::Context;
use derive_more::Debug;
use ratatui::{
    layout::{Constraint, Direction},
    prelude::Rect,
    Frame,
};
use serde_json_path::JsonPath;

/// Display text body of a successful response
#[derive(Debug)]
pub struct ResponseContentBody {
    /// Response body text content. State cell allows us to reset this whenever
    /// the request changes
    #[debug(skip)]
    text_window: StateCell<StateKey, Component<TextWindow<String>>>,
    /// Expression used to filter the content of the body down
    query: Option<Query>,
    /// Where the user enters their body query
    #[debug(skip)]
    query_text_box: Component<TextBox>,
}

pub struct ResponseContentBodyProps<'a> {
    pub record: &'a RequestRecord,
    pub parsed_body: Option<&'a dyn ResponseContent>,
}

#[derive(Debug, PartialEq)]
struct StateKey {
    request_id: RequestId,
    query: Option<Query>,
}

/// Callback event from the query text box when user hits Enter
struct QuerySubmit(String);

impl ResponseContentBody {
    /// Get visible body text
    pub fn text(&self) -> Option<String> {
        self.text_window
            .get()
            .map(|text_window| text_window.inner().text().to_owned())
    }
}

impl Default for ResponseContentBody {
    fn default() -> Self {
        Self {
            text_window: Default::default(),
            query: Default::default(),
            query_text_box: TextBox::default()
                .with_focus(false)
                .with_placeholder("'/' to filter body with JSONPath")
                .with_validator(|text| JsonPath::parse(text).is_ok())
                // Callback triggers an event, so we can modify our own state
                .with_on_submit(|text_box, context| {
                    context.queue_event(Event::other(QuerySubmit(
                        text_box.text().to_owned(),
                    )))
                })
                .into(),
        }
    }
}

impl EventHandler for ResponseContentBody {
    fn update(&mut self, _context: &mut UpdateContext, event: Event) -> Update {
        match event {
            Event::Input {
                action: Some(Action::Search),
                ..
            } => self.query_text_box.focus(),
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
        if self.query_text_box.is_focused() {
            vec![self.query_text_box.as_child()]
        } else if let Some(text_window) = self.text_window.get_mut() {
            vec![text_window.as_child()]
        } else {
            vec![]
        }
    }
}

impl<'a> Draw<ResponseContentBodyProps<'a>> for ResponseContentBody {
    fn draw(
        &self,
        frame: &mut Frame,
        props: ResponseContentBodyProps,
        area: Rect,
    ) {
        let [body_area, query_area] = layout(
            area,
            Direction::Vertical,
            [Constraint::Min(0), Constraint::Length(1)],
        );

        // Draw the body
        let state_key = StateKey {
            request_id: props.record.id,
            query: self.query.clone(),
        };
        let text = self.text_window.get_or_update(state_key, || {
            init_text_window(
                props.record,
                props.parsed_body,
                self.query.as_ref(),
            )
        });
        text.draw(frame, (), body_area);

        self.query_text_box.draw(frame, (), query_area);
    }
}

fn init_text_window(
    record: &RequestRecord,
    parsed_body: Option<&dyn ResponseContent>,
    query: Option<&Query>,
) -> Component<TextWindow<String>> {
    // Query and prettify text if possible. This involves a lot of cloning
    // because it makes stuff easier. If it becomes a bottleneck on large
    // responses it's fixable.
    let body = parsed_body
        .map(|parsed_body| {
            // Body is a known content type so we parsed it - apply a query if
            // necessary and prettify the output
            query
                .map(|query| query.query(parsed_body).prettify())
                .unwrap_or_else(|| parsed_body.prettify())
        })
        // Content couldn't be parsed, fall back to the raw text
        // If the text isn't UTF-8, we'll show a placeholder instead
        .unwrap_or_else(|| MaybeStr(record.response.body.bytes()).to_string());

    TextWindow::new(body).into()
}
