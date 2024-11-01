//! Display for HTTP responses

use crate::{
    message::Message,
    view::{
        common::{actions::ActionsModal, header_table::HeaderTable},
        component::queryable_body::{QueryableBody, QueryableBodyProps},
        context::UpdateContext,
        draw::{Draw, DrawMetadata, Generate, ToStringGenerate},
        event::{Child, Event, EventHandler, Update},
        state::StateCell,
        util::persistence::PersistedLazy,
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
    pub response: &'a ResponseRecord,
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
    request_id: RequestId,
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
    fn update(&mut self, _: &mut UpdateContext, event: Event) -> Update {
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
                    // Use whatever text is visible to the user. This differs
                    // from saving the body, because:
                    // 1. We need an owned string no matter what, so there's no
                    //   point in avoiding the allocation
                    // 2. We can't copy binary content, so if the file is binary
                    //   we'll copy the hexcode text
                    if let Some(body) = self
                        .state
                        .get()
                        .map(|state| state.body.data().visible_text())
                    {
                        ViewContext::send_message(Message::CopyText(body));
                    }
                }
                BodyMenuAction::SaveBody => {
                    if let Some(state) = self.state.get() {
                        // This will trigger a modal to ask the user for a path
                        ViewContext::send_message(Message::SaveResponseBody {
                            request_id: state.request_id,
                            data: state.body.data().parsed_text(),
                        });
                    }
                }
            }
        } else {
            return Update::Propagate(event);
        }
        Update::Consumed
    }

    fn children(&mut self) -> Vec<Component<Child<'_>>> {
        if let Some(state) = self.state.get_mut() {
            vec![state.body.to_child_mut()]
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
        let state = self.state.get_or_update(&props.request_id, || State {
            request_id: props.request_id,
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
        test_util::{
            harness, terminal, TestHarness, TestResponseParser, TestTerminal,
        },
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
        ResponseRecord {
            headers: header_map(indexmap! {"content-type" => "application/json"}),
            body: br#"{"hello":"world"}"#.to_vec().into(),
            ..ResponseRecord::factory(())
        },
        "{\n  \"hello\": \"world\"\n}",
    )]
    #[case::unparsed_text_body(
        ResponseRecord {
            headers: header_map(indexmap! {"content-type" => "text/plain"}),
            body: b"hello!".to_vec().into(),
            ..ResponseRecord::factory(())
        },
        "hello!\n",
    )]
    #[case::binary_body(
        ResponseRecord {
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
        let mut exchange = Exchange {
            response,
            ..Exchange::factory(())
        };
        TestResponseParser::parse_body(&mut exchange.response);
        let mut component = TestComponent::new(
            &harness,
            &terminal,
            ResponseBodyView::default(),
            ResponseBodyViewProps {
                request_id: exchange.id,
                recipe_id: &exchange.request.recipe_id,
                response: &exchange.response,
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
        ResponseRecord {
            headers: header_map(indexmap! {"content-type" => "application/json"}),
            body: br#"{"hello":"world"}"#.to_vec().into(),
            ..ResponseRecord::factory(())
        },
        Some("{\n  \"hello\": \"world\"\n}"),
    )]
    #[case::unparsed_text_body(
        ResponseRecord {
            headers: header_map(indexmap! {"content-type" => "text/plain"}),
            body: b"hello!".to_vec().into(),
            ..ResponseRecord::factory(())
        },
        None,
    )]
    #[case::binary_body(
        ResponseRecord {
            body: b"\x01\x02\x03".to_vec().into(),
            ..ResponseRecord::factory(())
        },
        None,
    )]
    #[tokio::test]
    async fn test_save_file(
        mut harness: TestHarness,
        terminal: TestTerminal,
        #[case] response: ResponseRecord,
        #[case] expected_body: Option<&str>,
    ) {
        let mut exchange = Exchange {
            response,
            ..Exchange::factory(())
        };
        TestResponseParser::parse_body(&mut exchange.response);
        let mut component = TestComponent::new(
            &harness,
            &terminal,
            ResponseBodyView::default(),
            ResponseBodyViewProps {
                request_id: exchange.id,
                recipe_id: &exchange.request.recipe_id,
                response: &exchange.response,
            },
        );

        component
            .update_draw(Event::new_local(BodyMenuAction::SaveBody))
            .assert_empty();

        let (request_id, data) = assert_matches!(
            harness.pop_message_now(),
            Message::SaveResponseBody { request_id, data } => (request_id, data),
        );
        assert_eq!(request_id, exchange.id);
        assert_eq!(data.as_deref(), expected_body);
    }
}
