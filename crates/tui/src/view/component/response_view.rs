//! Display for HTTP responses

use crate::{
    context::TuiContext,
    message::Message,
    view::{
        common::{
            actions::ActionsModal, header_table::HeaderTable,
            modal::ModalHandle,
        },
        component::queryable_body::QueryableBody,
        context::UpdateContext,
        draw::{Draw, DrawMetadata, Generate, ToStringGenerate},
        event::{Child, Event, EventHandler, Update},
        state::StateCell,
        util::{persistence::PersistedLazy, view_text},
        Component, ViewContext,
    },
};
use derive_more::Display;
use persisted::PersistedKey;
use ratatui::{text::Text, Frame};
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
    actions_handle: ModalHandle<ActionsModal<BodyMenuAction>>,
}

#[derive(Clone)]
pub struct ResponseBodyViewProps<'a> {
    pub request_id: RequestId,
    pub recipe_id: &'a RecipeId,
    pub response: &'a Arc<ResponseRecord>,
}

/// Items in the actions popup menu for the Body
#[derive(
    Copy, Clone, Debug, Default, Display, EnumCount, EnumIter, PartialEq,
)]
enum BodyMenuAction {
    #[default]
    #[display("Edit Collection")]
    EditCollection,
    #[display("View Body")]
    ViewBody,
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

impl ResponseBodyView {
    fn with_body(&self, f: impl Fn(&Text)) {
        if let Some(state) = self.state.get() {
            f(state.body.data().visible_text())
        }
    }
}

impl EventHandler for ResponseBodyView {
    fn update(&mut self, _: &mut UpdateContext, event: Event) -> Update {
        if let Some(Action::OpenActions) = event.action() {
            self.actions_handle.open(ActionsModal::default());
        } else if let Some(menu_action) = self.actions_handle.emitted(&event) {
            match menu_action {
                BodyMenuAction::EditCollection => {
                    ViewContext::send_message(Message::CollectionEdit)
                }
                BodyMenuAction::ViewBody => self.with_body(view_text),
                BodyMenuAction::CopyBody => {
                    // Use whatever text is visible to the user. This differs
                    // from saving the body, because:
                    // 1. We need an owned string no matter what, so there's no
                    //   point in avoiding the allocation
                    // 2. We can't copy binary content, so if the file is binary
                    //   we'll copy the hexcode text
                    self.with_body(|body| {
                        ViewContext::send_message(Message::CopyText(
                            body.to_string(),
                        ));
                    });
                }
                BodyMenuAction::SaveBody => {
                    if let Some(state) = self.state.get() {
                        // This will trigger a modal to ask the user for a path
                        ViewContext::send_message(Message::SaveResponseBody {
                            request_id: state.request_id,
                            data: state.body.data().modified_text(),
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
        let state = self.state.get_or_update(&props.request_id, || State {
            request_id: props.request_id,
            body: PersistedLazy::new(
                ResponseQueryPersistedKey(props.recipe_id.clone()),
                QueryableBody::new(
                    Arc::clone(props.response),
                    TuiContext::get().config.commands.query_default.clone(),
                ),
            )
            .into(),
        });

        state.body.draw(frame, (), metadata.area(), true);
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
    use crossterm::event::KeyCode;
    use indexmap::indexmap;
    use rstest::rstest;
    use slumber_core::{
        assert_matches,
        http::Exchange,
        test_util::{header_map, Factory},
    };

    /// Test "Copy Body" menu action
    #[rstest]
    #[case::text_body(
        ResponseRecord {
            body: br#"{"hello":"world"}"#.as_slice().into(),
            ..ResponseRecord::factory(())
        },
        "{\"hello\":\"world\"}",
    )]
    #[case::json_body(
        ResponseRecord {
            headers: header_map(indexmap! {"content-type" => "application/json"}),
            body: br#"{"hello":"world"}"#.as_slice().into(),
            ..ResponseRecord::factory(())
        },
        "{\n  \"hello\": \"world\"\n}",
    )]
    #[case::binary_body(
        ResponseRecord {
            body: b"\x01\x02\x03\xff".as_slice().into(),
            ..ResponseRecord::factory(())
        },
        "01 02 03 ff"
    )]
    #[tokio::test]
    async fn test_copy_body(
        mut harness: TestHarness,
        terminal: TestTerminal,
        #[case] response: ResponseRecord,
        #[case] expected_body: &str,
    ) {
        let exchange = Exchange {
            response: response.into(),
            ..Exchange::factory(())
        };
        let mut component = TestComponent::with_props(
            &harness,
            &terminal,
            ResponseBodyView::default(),
            ResponseBodyViewProps {
                request_id: exchange.id,
                recipe_id: &exchange.request.recipe_id,
                response: &exchange.response,
            },
        );

        // Open actions modal and select the copy action
        component
            .send_keys([
                KeyCode::Char('x'),
                KeyCode::Down,
                KeyCode::Down,
                KeyCode::Enter,
            ])
            .assert_empty();

        let body = assert_matches!(
            harness.pop_message_now(),
            Message::CopyText(body) => body,
        );
        assert_eq!(body, expected_body);
    }

    /// Test "Save Body as File" menu action
    #[rstest]
    #[case::text_body(
        ResponseRecord {
            body: b"hello!".as_slice().into(),
            ..ResponseRecord::factory(())
        },
        None,
        None,
    )]
    #[case::json_body(
        ResponseRecord {
            headers: header_map(indexmap! {"content-type" => "application/json"}),
            body: br#"{"hello":"world"}"#.as_slice().into(),
            ..ResponseRecord::factory(())
        },
        None,
        // Body has been prettified, so we can't use the original
        Some("{\n  \"hello\": \"world\"\n}"),
    )]
    #[case::binary_body(
        ResponseRecord {
            body: b"\x01\x02\x03".as_slice().into(),
            ..ResponseRecord::factory(())
        },
        None,
        None,
    )]
    #[case::queried_body(
        ResponseRecord {
            body: b"hello!".as_slice().into(),
            ..ResponseRecord::factory(())
        },
        Some("head -c 4"),
        Some("hell"),
    )]
    #[tokio::test]
    async fn test_save_file(
        mut harness: TestHarness,
        terminal: TestTerminal,
        #[case] response: ResponseRecord,
        #[case] query: Option<&str>,
        #[case] expected_body: Option<&str>,
    ) {
        use crate::test_util::run_local;

        let exchange = Exchange {
            response: response.into(),
            ..Exchange::factory(())
        };
        let mut component = TestComponent::with_props(
            &harness,
            &terminal,
            ResponseBodyView::default(),
            ResponseBodyViewProps {
                request_id: exchange.id,
                recipe_id: &exchange.request.recipe_id,
                response: &exchange.response,
            },
        );

        if let Some(query) = query {
            // Querying requires a LocalSet to run the command in the background
            run_local(async {
                // Type something into the query box
                component.send_key(KeyCode::Char('/')).assert_empty();
                component.send_text(query).assert_empty();
                component.send_key(KeyCode::Enter).assert_empty();
                // Wait for the command to finish, pass results back to the
                // component
            })
            .await;
            component.drain_draw().assert_empty();
        }

        // Open actions modal and select the save action
        component
            .send_keys([
                KeyCode::Char('x'),
                KeyCode::Down,
                KeyCode::Down,
                KeyCode::Down,
                KeyCode::Enter,
            ])
            .assert_empty();

        let (request_id, data) = assert_matches!(
            harness.pop_message_now(),
            Message::SaveResponseBody { request_id, data } => (request_id, data),
        );
        assert_eq!(request_id, exchange.id);
        assert_eq!(data.as_deref(), expected_body);
    }
}
