//! Display for HTTP responses

use crate::{
    message::Message,
    view::{
        common::{actions::ActionsModal, header_table::HeaderTable},
        component::queryable_body::{QueryableBody, QueryableBodyProps},
        context::PersistedLazy,
        draw::{Draw, DrawMetadata, Generate, ToStringGenerate},
        event::{Event, EventHandler, Update},
        state::StateCell,
        Component, ViewContext,
    },
};
use derive_more::Display;
use persisted::PersistedKey;
use ratatui::Frame;
use serde::Serialize;
use slumber_config::Action;
use slumber_core::{
    collection::RecipeId,
    http::{RequestId, ResponseRecord},
};
use std::sync::Arc;
use strum::{EnumCount, EnumIter};

/// Display response body
#[derive(Debug, Default)]
pub struct ResponseBodyView {
    /// Persist the response body to track view state. Update whenever the
    /// loaded request changes
    state: StateCell<RequestId, State>,
}

#[derive(Clone)]
pub struct ResponseBodyViewProps<'a> {
    pub request_id: RequestId,
    pub recipe_id: &'a RecipeId,
    pub response: Arc<ResponseRecord>,
}

/// Items in the actions popup menu for the Body
#[derive(
    Copy, Clone, Debug, Default, Display, EnumCount, EnumIter, PartialEq,
)]
enum BodyMenuAction {
    #[default]
    #[display("Edit Collection")]
    EditCollection,
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
    response: Arc<ResponseRecord>,
    /// The presentable version of the response body, which may or may not
    /// match the response body. We apply transformations such as filter,
    /// prettification, or in the case of binary responses, a hex dump.
    body: Component<PersistedLazy<ResponseQueryPersistedKey, QueryableBody>>,
}

/// Persisted key for response body JSONPath query text box
#[derive(Debug, Serialize, PersistedKey)]
#[persisted(String)]
struct ResponseQueryPersistedKey(RecipeId);

impl EventHandler for ResponseBodyView {
    fn update(&mut self, event: Event) -> Update {
        if let Some(Action::OpenActions) = event.action() {
            ViewContext::open_modal::<ActionsModal<BodyMenuAction>>(
                Default::default(),
            );
        } else if let Some(action) = event.local::<BodyMenuAction>() {
            match action {
                BodyMenuAction::EditCollection => {
                    ViewContext::send_message(Message::CollectionEdit)
                }
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

impl<'a> Draw<ResponseBodyViewProps<'a>> for ResponseBodyView {
    fn draw(
        &self,
        frame: &mut Frame,
        props: ResponseBodyViewProps,
        metadata: DrawMetadata,
    ) {
        let response = &props.response;
        let state = self.state.get_or_update(props.request_id, || State {
            response: Arc::clone(&props.response),
            body: PersistedLazy::new(
                ResponseQueryPersistedKey(props.recipe_id.clone()),
                QueryableBody::new(),
            )
            .into(),
        });

        state.body.draw(
            frame,
            QueryableBodyProps {
                content_type: response.content_type(),
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
    pub response: &'a ResponseRecord,
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
        test_util::{harness, terminal, TestHarness, TestTerminal},
        view::test_util::TestComponent,
    };
    use indexmap::indexmap;
    use rstest::rstest;
    use slumber_core::{
        assert_matches,
        http::Exchange,
        test_util::{header_map, Factory},
    };

    /// Test "Copy Body" menu action
    #[rstest]
    #[case::json_body(
        ResponseRecord{
            headers: header_map(indexmap! {"content-type" => "application/json"}),
            body: br#"{"hello":"world"}"#.to_vec().into(),
            ..ResponseRecord::factory(())
        },
        // Body gets prettified
        "{\n  \"hello\": \"world\"\n}"
    )]
    #[case::binary_body(
        ResponseRecord{
            headers: header_map(indexmap! {"content-type" => "image/png"}),
            body: b"\x01\x02\x03\xff".to_vec().into(),
            ..ResponseRecord::factory(())
        },
        // Remove newline after this fix:
        // https://github.com/ratatui-org/ratatui/pull/1320
        "01 02 03 ff\n"
    )]
    #[tokio::test]
    async fn test_copy_body(
        mut harness: TestHarness,
        terminal: TestTerminal,
        #[case] response: ResponseRecord,
        #[case] expected_body: &str,
    ) {
        response.parse_body(); // Normally the view does this
        let exchange = Exchange {
            response: response.into(),
            ..Exchange::factory(())
        };
        let mut component = TestComponent::new(
            &terminal,
            ResponseBodyView::default(),
            ResponseBodyViewProps {
                request_id: exchange.id,
                recipe_id: &exchange.request.recipe_id,
                response: exchange.response,
            },
        );

        component
            .update_draw(Event::new_local(BodyMenuAction::CopyBody))
            .assert_empty();

        let body = assert_matches!(
            harness.pop_message_now(),
            Message::CopyText(body) => body,
        );
        assert_eq!(body, expected_body);
    }

    /// Test "Save Body as File" menu action
    #[rstest]
    #[case::json_body(
        ResponseRecord{
            headers: header_map(indexmap! {"content-type" => "application/json"}),
            body: br#"{"hello":"world"}"#.to_vec().into(),
            ..ResponseRecord::factory(())
        },
        // Body gets prettified
        b"{\n  \"hello\": \"world\"\n}",
        "data.json"
    )]
    #[case::binary_body(
        ResponseRecord{
            headers: header_map(indexmap! {"content-type" => "image/png"}),
            body: b"\x01\x02\x03".to_vec().into(),
            ..ResponseRecord::factory(())
        },
        b"\x01\x02\x03",
        "data.png"
    )]
    #[case::content_disposition(
        ResponseRecord{
            headers: header_map(indexmap! {
                "content-type" => "image/png",
                "content-disposition" => "attachment; filename=\"dogs.png\"",
            }),
            body: b"\x01\x02\x03".to_vec().into(),
            ..ResponseRecord::factory(())
        },
        b"\x01\x02\x03",
        "dogs.png"
    )]
    #[tokio::test]
    async fn test_save_file(
        mut harness: TestHarness,
        terminal: TestTerminal,
        #[case] response: ResponseRecord,
        #[case] expected_body: &[u8],
        #[case] expected_path: &str,
    ) {
        response.parse_body(); // Normally the view does this
        let exchange = Exchange {
            response: response.into(),
            ..Exchange::factory(())
        };
        let mut component = TestComponent::new(
            &terminal,
            ResponseBodyView::default(),
            ResponseBodyViewProps {
                request_id: exchange.id,
                recipe_id: &exchange.request.recipe_id,
                response: exchange.response,
            },
        );

        component
            .update_draw(Event::new_local(BodyMenuAction::SaveBody))
            .assert_empty();

        let (data, default_path) = assert_matches!(
            harness.pop_message_now(),
            Message::SaveFile { data, default_path } => (data, default_path),
        );
        assert_eq!(data, expected_body);
        assert_eq!(default_path.as_deref(), Some(expected_path));
    }
}
