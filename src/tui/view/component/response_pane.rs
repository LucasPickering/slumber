use crate::{
    http::{RequestId, RequestRecord, Response},
    tui::{
        context::TuiContext,
        input::Action,
        message::{Message, MessageSender},
        view::{
            common::{
                actions::ActionsModal, header_table::HeaderTable, tabs::Tabs,
                Pane,
            },
            component::record_body::{RecordBody, RecordBodyProps},
            draw::{Draw, Generate, ToStringGenerate},
            event::{Event, EventHandler, EventQueue, Update},
            state::{persistence::PersistentKey, RequestState, StateCell},
            Component,
        },
    },
};
use chrono::Utc;
use derive_more::{Debug, Display};
use ratatui::{
    layout::Layout,
    prelude::{Alignment, Constraint, Rect},
    text::Line,
    widgets::{Paragraph, Wrap},
    Frame,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use strum::{EnumCount, EnumIter};

/// Display HTTP response state, which could be in progress, complete, or
/// failed. This can be used in both a paned and fullscreen view.
#[derive(Debug, Default)]
pub struct ResponsePane {
    content: Component<CompleteResponseContent>,
}

pub struct ResponsePaneProps<'a> {
    pub is_selected: bool,
    pub active_request: Option<&'a RequestState>,
}

/// Items in the actions popup menu
#[derive(Copy, Clone, Debug, Display, EnumCount, EnumIter, PartialEq)]
enum MenuAction {
    #[display("Copy Body")]
    CopyBody,
    #[display("Save Body as File")]
    SaveBody,
}

impl ToStringGenerate for MenuAction {}

impl EventHandler for ResponsePane {
    fn children(&mut self) -> Vec<Component<&mut dyn EventHandler>> {
        vec![self.content.as_child()]
    }
}

impl<'a> Draw<ResponsePaneProps<'a>> for ResponsePane {
    fn draw(
        &self,
        frame: &mut Frame,
        props: ResponsePaneProps<'a>,
        area: Rect,
    ) {
        // Render outermost block
        let title = TuiContext::get()
            .input_engine
            .add_hint("Response", Action::SelectResponse);
        let block = Pane {
            title: &title,
            is_focused: props.is_selected,
        };
        let block = block.generate();
        frame.render_widget(&block, area);
        let area = block.inner(area);

        match props.active_request {
            None | Some(RequestState::BuildError { .. }) => {}
            Some(RequestState::Building { .. }) => {
                frame.render_widget(Paragraph::new("Loading..."), area)
            }
            Some(RequestState::Loading { start_time, .. }) => {
                frame.render_widget(Paragraph::new("Loading..."), area);
                let duration = Utc::now() - start_time;
                frame.render_widget(
                    Paragraph::new(duration.generate())
                        .alignment(Alignment::Right),
                    area,
                );
            }

            Some(RequestState::Response { record }) => self.content.draw(
                frame,
                CompleteResponseContentProps { record },
                area,
            ),

            // Sadge
            Some(RequestState::RequestError { error }) => frame.render_widget(
                Paragraph::new(error.generate()).wrap(Wrap::default()),
                area,
            ),
        }
    }
}

/// Display response success state (tab container)
#[derive(Debug)]
struct CompleteResponseContent {
    #[debug(skip)]
    tabs: Component<Tabs<Tab>>,
    /// Persist the response body to track view state. Update whenever the
    /// loaded request changes
    #[debug(skip)]
    state: StateCell<RequestId, State>,
}

struct CompleteResponseContentProps<'a> {
    record: &'a RequestRecord,
}

/// Internal state
struct State {
    /// Use Arc so we're not cloning large responses
    response: Arc<Response>,
    /// The presentable version of the response body, which may or may not
    /// match the response body. We apply transformations such as filter,
    /// prettification, or in the case of binary responses, a hex dump.
    body: Component<RecordBody>,
}

#[derive(
    Copy,
    Clone,
    Debug,
    Default,
    Display,
    EnumCount,
    EnumIter,
    PartialEq,
    Serialize,
    Deserialize,
)]
enum Tab {
    #[default]
    Body,
    Headers,
}

impl CompleteResponseContent {}

impl EventHandler for CompleteResponseContent {
    fn update(&mut self, messages_tx: &MessageSender, event: Event) -> Update {
        if let Some(Action::OpenActions) = event.action() {
            EventQueue::open_modal_default::<ActionsModal<MenuAction>>();
            Update::Consumed
        } else if let Some(action) = event.other::<MenuAction>() {
            match action {
                MenuAction::CopyBody => {
                    // Use whatever text is visible to the user
                    if let Some(body) =
                        self.state.get().and_then(|state| state.body.text())
                    {
                        messages_tx.send(Message::CopyText(body));
                    }
                }
                MenuAction::SaveBody => {
                    // For text, use whatever is visible to the user. For
                    // binary, use the raw value
                    if let Some(state) = self.state.get() {
                        // If we've parsed the body, then save exactly what the
                        // user sees. Otherwise, save the raw bytes. This is
                        // going to clone the whole body, which could be big.
                        // If we need to optimize this, we would have to shove
                        // all querying to the main data storage, so the main
                        // loop can access it directly to be written.
                        let data = if state.response.body.parsed().is_some() {
                            state.body.text().unwrap_or_default().into_bytes()
                        } else {
                            state.response.body.bytes().to_vec()
                        };

                        // This will trigger a modal to ask the user for a path
                        messages_tx.send(Message::SaveFile {
                            default_path: state.response.file_name(),
                            data,
                        });
                    }
                }
            }
            Update::Consumed
        } else {
            Update::Propagate(event)
        }
    }

    fn children(&mut self) -> Vec<Component<&mut dyn EventHandler>> {
        let selected_tab = *self.tabs.selected();
        let mut children = vec![];
        match selected_tab {
            Tab::Body => {
                if let Some(state) = self.state.get_mut() {
                    children.push(state.body.as_child());
                }
            }
            Tab::Headers => {}
        }
        // Tabs goes last, because pane content gets priority
        children.push(self.tabs.as_child());
        children
    }
}

impl<'a> Draw<CompleteResponseContentProps<'a>> for CompleteResponseContent {
    fn draw(
        &self,
        frame: &mut Frame,
        props: CompleteResponseContentProps,
        area: Rect,
    ) {
        let response = &props.record.response;
        // Set response state regardless of what tab we're on, so we always
        // have access to it
        let state = self.state.get_or_update(props.record.id, || State {
            response: Arc::clone(&props.record.response),
            body: Default::default(),
        });

        // Split the main area again to allow tabs
        let [header_area, tabs_area, content_area] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .areas(area);

        // Metadata
        frame.render_widget(response.status.generate(), header_area);
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                props.record.response.body.size().to_string_as(false).into(),
                " / ".into(),
                props.record.duration().generate(),
            ]))
            .alignment(Alignment::Right),
            header_area,
        );

        // Navigation tabs
        self.tabs.draw(frame, (), tabs_area);

        // Main content for the response
        match self.tabs.selected() {
            Tab::Body => {
                state.body.draw(
                    frame,
                    RecordBodyProps {
                        body: &response.body,
                    },
                    content_area,
                );
            }

            Tab::Headers => frame.render_widget(
                HeaderTable {
                    headers: &response.headers,
                }
                .generate(),
                content_area,
            ),
        }
    }
}

impl Default for CompleteResponseContent {
    fn default() -> Self {
        Self {
            tabs: Tabs::new(PersistentKey::ResponseTab).into(),
            state: Default::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::*;
    use indexmap::indexmap;
    use ratatui::{backend::TestBackend, Terminal};
    use rstest::rstest;

    /// Test "Copy Body" menu action
    #[rstest]
    #[case::json_body(
        Response{
            headers: header_map(indexmap! {"content-type" => "application/json"}),
            body: br#"{"hello":"world"}"#.to_vec().into(),
            ..Response::factory()
        },
        // Body gets prettified
        "{\n  \"hello\": \"world\"\n}"
    )]
    #[case::binary_body(
        Response{
            headers: header_map(indexmap! {"content-type" => "image/png"}),
            body: b"\x01\x02\x03\xff".to_vec().into(),
            ..Response::factory()
        },
        "01 02 03 ff"
    )]
    #[tokio::test]
    async fn test_copy_body(
        _tui_context: (),
        mut messages: MessageQueue,
        mut terminal: Terminal<TestBackend>,
        #[case] response: Response,
        #[case] expected_body: &str,
    ) {
        // Draw once to initialize state
        let mut component = CompleteResponseContent::default();
        response.parse_body(); // Normally the view does this
        let record = RequestRecord {
            response: response.into(),
            ..RequestRecord::factory()
        };
        component.draw(
            &mut terminal.get_frame(),
            CompleteResponseContentProps { record: &record },
            Rect::default(),
        );

        let update = component
            .update(messages.tx(), Event::new_other(MenuAction::CopyBody));
        // unstable: https://github.com/rust-lang/rust/issues/82775
        assert!(matches!(update, Update::Consumed));

        let message = messages.pop_now();
        let Message::CopyText(body) = &message else {
            panic!("Wrong message: {message:?}")
        };
        assert_eq!(body, expected_body);
    }

    /// Test "Save Body as File" menu action
    #[rstest]
    #[case::json_body(
        Response{
            headers: header_map(indexmap! {"content-type" => "application/json"}),
            body: br#"{"hello":"world"}"#.to_vec().into(),
            ..Response::factory()
        },
        // Body gets prettified
        b"{\n  \"hello\": \"world\"\n}",
        "data.json"
    )]
    #[case::binary_body(
        Response{
            headers: header_map(indexmap! {"content-type" => "image/png"}),
            body: b"\x01\x02\x03".to_vec().into(),
            ..Response::factory()
        },
        b"\x01\x02\x03",
        "data.png"
    )]
    #[case::content_disposition(
        Response{
            headers: header_map(indexmap! {
                "content-type" => "image/png",
                "content-disposition" => "attachment; filename=\"dogs.png\"",
            }),
            body: b"\x01\x02\x03".to_vec().into(),
            ..Response::factory()
        },
        b"\x01\x02\x03",
        "dogs.png"
    )]
    #[tokio::test]
    async fn test_save_file(
        _tui_context: (),
        mut messages: MessageQueue,
        mut terminal: Terminal<TestBackend>,
        #[case] response: Response,
        #[case] expected_body: &[u8],
        #[case] expected_path: &str,
    ) {
        let mut component = CompleteResponseContent::default();
        response.parse_body(); // Normally the view does this
        let record = RequestRecord {
            response: response.into(),
            ..RequestRecord::factory()
        };

        // Draw once to initialize state
        component.draw(
            &mut terminal.get_frame(),
            CompleteResponseContentProps { record: &record },
            Rect::default(),
        );

        let update = component
            .update(messages.tx(), Event::new_other(MenuAction::SaveBody));
        // unstable: https://github.com/rust-lang/rust/issues/82775
        assert!(matches!(update, Update::Consumed));

        let message = messages.pop_now();
        let Message::SaveFile { data, default_path } = &message else {
            panic!("Wrong message: {message:?}")
        };
        assert_eq!(data, expected_body);
        assert_eq!(default_path.as_deref(), Some(expected_path));
    }
}
