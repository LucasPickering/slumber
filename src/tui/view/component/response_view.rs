//! Display for HTTP responses

use crate::{
    http::{RequestId, Response},
    tui::{
        input::Action,
        message::Message,
        view::{
            common::{actions::ActionsModal, header_table::HeaderTable},
            component::record_body::{RecordBody, RecordBodyProps},
            draw::{Draw, DrawMetadata, Generate, ToStringGenerate},
            event::{Event, EventHandler, Update},
            state::StateCell,
            Component, ViewContext,
        },
    },
};
use derive_more::Display;
use ratatui::Frame;
use std::sync::Arc;
use strum::{EnumCount, EnumIter};

/// Display response body
#[derive(Debug, Default)]
pub struct ResponseBodyView {
    /// Persist the response body to track view state. Update whenever the
    /// loaded request changes
    state: StateCell<RequestId, State>,
}

pub struct ResponseBodyViewProps {
    pub request_id: RequestId,
    pub response: Arc<Response>,
}

/// Items in the actions popup menu for the Body
#[derive(Copy, Clone, Debug, Display, EnumCount, EnumIter, PartialEq)]
enum BodyMenuAction {
    #[display("Copy Body")]
    CopyBody,
    #[display("Save Body as File")]
    SaveBody,
}

impl ToStringGenerate for BodyMenuAction {}

/// Internal state
#[derive(Debug)]
struct State {
    /// Use Arc so we're not cloning large responses
    response: Arc<Response>,
    /// The presentable version of the response body, which may or may not
    /// match the response body. We apply transformations such as filter,
    /// prettification, or in the case of binary responses, a hex dump.
    body: Component<RecordBody>,
}

impl EventHandler for ResponseBodyView {
    fn update(&mut self, event: Event) -> Update {
        if let Some(Action::OpenActions) = event.action() {
            ViewContext::open_modal_default::<ActionsModal<BodyMenuAction>>();
        } else if let Some(action) = event.other::<BodyMenuAction>() {
            match action {
                BodyMenuAction::CopyBody => {
                    // Use whatever text is visible to the user
                    if let Some(body) = self
                        .state
                        .get()
                        .and_then(|state| state.body.data().text())
                    {
                        ViewContext::send_message(Message::CopyText(body));
                    }
                }
                BodyMenuAction::SaveBody => {
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
                            state
                                .body
                                .data()
                                .text()
                                .unwrap_or_default()
                                .into_bytes()
                        } else {
                            state.response.body.bytes().to_vec()
                        };

                        // This will trigger a modal to ask the user for a path
                        ViewContext::send_message(Message::SaveFile {
                            default_path: state.response.file_name(),
                            data,
                        });
                    }
                }
            }
        } else {
            return Update::Propagate(event);
        }
        Update::Consumed
    }

    fn children(&mut self) -> Vec<Component<&mut dyn EventHandler>> {
        if let Some(state) = self.state.get_mut() {
            vec![state.body.as_child()]
        } else {
            vec![]
        }
    }
}

impl Draw<ResponseBodyViewProps> for ResponseBodyView {
    fn draw(
        &self,
        frame: &mut Frame,
        props: ResponseBodyViewProps,
        metadata: DrawMetadata,
    ) {
        let response = &props.response;
        let state = self.state.get_or_update(props.request_id, || State {
            response: Arc::clone(&props.response),
            body: Default::default(),
        });

        state.body.draw(
            frame,
            RecordBodyProps {
                body: &response.body,
            },
            metadata.area(),
            true,
        );
    }
}

#[derive(Debug, Default)]
pub struct ResponseHeadersView;

pub struct ResponseHeadersViewProps<'a> {
    pub response: &'a Response,
}

impl<'a> Draw<ResponseHeadersViewProps<'a>> for ResponseHeadersView {
    fn draw(
        &self,
        frame: &mut Frame,
        props: ResponseHeadersViewProps,
        metadata: DrawMetadata,
    ) {
        frame.render_widget(
            HeaderTable {
                headers: &props.response.headers,
            }
            .generate(),
            metadata.area(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        db::CollectionDatabase, http::RequestRecord, test_util::*,
        tui::context::TuiContext,
    };
    use indexmap::indexmap;
    use ratatui::{backend::TestBackend, Terminal};
    use rstest::rstest;

    /// Test "Copy Body" menu action
    #[rstest]
    #[case::json_body(
        Response{
            headers: header_map(indexmap! {"content-type" => "application/json"}),
            body: br#"{"hello":"world"}"#.to_vec().into(),
            ..Response::factory(())
        },
        // Body gets prettified
        "{\n  \"hello\": \"world\"\n}"
    )]
    #[case::binary_body(
        Response{
            headers: header_map(indexmap! {"content-type" => "image/png"}),
            body: b"\x01\x02\x03\xff".to_vec().into(),
            ..Response::factory(())
        },
        "01 02 03 ff"
    )]
    #[tokio::test]
    async fn test_copy_body(
        _tui_context: &TuiContext,
        database: CollectionDatabase,
        mut messages: MessageQueue,
        mut terminal: Terminal<TestBackend>,
        #[case] response: Response,
        #[case] expected_body: &str,
    ) {
        ViewContext::init(database.clone(), messages.tx().clone());
        // Draw once to initialize state
        let mut component = ResponseBodyView::default();
        response.parse_body(); // Normally the view does this
        let record = RequestRecord {
            response: response.into(),
            ..RequestRecord::factory(())
        };
        component.draw(
            &mut terminal.get_frame(),
            ResponseBodyViewProps {
                request_id: record.id,
                response: record.response,
            },
            DrawMetadata::default(),
        );

        let update =
            component.update(Event::new_other(BodyMenuAction::CopyBody));
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
            ..Response::factory(())
        },
        // Body gets prettified
        b"{\n  \"hello\": \"world\"\n}",
        "data.json"
    )]
    #[case::binary_body(
        Response{
            headers: header_map(indexmap! {"content-type" => "image/png"}),
            body: b"\x01\x02\x03".to_vec().into(),
            ..Response::factory(())
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
            ..Response::factory(())
        },
        b"\x01\x02\x03",
        "dogs.png"
    )]
    #[tokio::test]
    async fn test_save_file(
        _tui_context: &TuiContext,
        database: CollectionDatabase,
        mut messages: MessageQueue,
        mut terminal: Terminal<TestBackend>,
        #[case] response: Response,
        #[case] expected_body: &[u8],
        #[case] expected_path: &str,
    ) {
        ViewContext::init(database.clone(), messages.tx().clone());
        let mut component = ResponseBodyView::default();
        response.parse_body(); // Normally the view does this
        let record = RequestRecord {
            response: response.into(),
            ..RequestRecord::factory(())
        };

        // Draw once to initialize state
        component.draw(
            &mut terminal.get_frame(),
            ResponseBodyViewProps {
                request_id: record.id,
                response: record.response,
            },
            DrawMetadata::default(),
        );

        let update =
            component.update(Event::new_other(BodyMenuAction::SaveBody));
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
