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
        event::{Child, Event, EventHandler, OptionEvent},
        util::{persistence::PersistedLazy, view_text},
        Component, ViewContext,
    },
};
use derive_more::Display;
use persisted::PersistedKey;
use ratatui::Frame;
use serde::Serialize;
use slumber_config::Action;
use slumber_core::{collection::RecipeId, http::ResponseRecord};
use std::sync::Arc;
use strum::{EnumCount, EnumIter};

/// Display response body
#[derive(Debug)]
pub struct ResponseBodyView {
    response: Arc<ResponseRecord>,
    /// The presentable version of the response body, which may or may not
    /// match the response body. We apply transformations such as filter,
    /// prettification, or in the case of binary responses, a hex dump.
    body: Component<PersistedLazy<ResponseQueryPersistedKey, QueryableBody>>,
    actions_handle: ModalHandle<ActionsModal<BodyMenuAction>>,
}

impl ResponseBodyView {
    pub fn new(recipe_id: RecipeId, response: Arc<ResponseRecord>) -> Self {
        let body = PersistedLazy::new(
            ResponseQueryPersistedKey(recipe_id),
            QueryableBody::new(
                Arc::clone(&response),
                TuiContext::get().config.commands.default_query.clone(),
            ),
        )
        .into();
        Self {
            response,
            body,
            actions_handle: Default::default(),
        }
    }
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

/// Persisted key for response body JSONPath query text box
#[derive(Debug, Serialize, PersistedKey)]
#[persisted(String)]
struct ResponseQueryPersistedKey(RecipeId);

impl EventHandler for ResponseBodyView {
    fn update(&mut self, _: &mut UpdateContext, event: Event) -> Option<Event> {
        event
            .opt()
            .action(|action, propagate| match action {
                Action::OpenActions => {
                    self.actions_handle.open(ActionsModal::default())
                }
                _ => propagate.set(),
            })
            .emitted(self.actions_handle, |menu_action| match menu_action {
                BodyMenuAction::EditCollection => {
                    ViewContext::send_message(Message::CollectionEdit)
                }
                BodyMenuAction::ViewBody => {
                    view_text(
                        self.body.data().visible_text(),
                        self.response.mime(),
                    );
                }
                BodyMenuAction::CopyBody => {
                    // Use whatever text is visible to the user. This differs
                    // from saving the body, because we can't copy binary
                    // content, so if the file is binary we'll copy the hexcode
                    // text
                    ViewContext::send_message(Message::CopyText(
                        self.body.data().visible_text().to_string(),
                    ));
                }
                BodyMenuAction::SaveBody => {
                    // This will trigger a modal to ask the user for a path
                    ViewContext::send_message(Message::SaveResponseBody {
                        request_id: self.response.id,
                        data: self.body.data().modified_text(),
                    });
                }
            })
    }

    fn children(&mut self) -> Vec<Component<Child<'_>>> {
        vec![self.body.to_child_mut()]
    }
}

impl Draw for ResponseBodyView {
    fn draw(&self, frame: &mut Frame, _: (), metadata: DrawMetadata) {
        self.body.draw(frame, (), metadata.area(), true);
    }
}

#[derive(Debug)]
pub struct ResponseHeadersView {
    response: Arc<ResponseRecord>,
}

impl ResponseHeadersView {
    pub fn new(response: Arc<ResponseRecord>) -> Self {
        Self { response }
    }
}

impl EventHandler for ResponseHeadersView {}

impl Draw for ResponseHeadersView {
    fn draw(&self, frame: &mut Frame, _: (), metadata: DrawMetadata) {
        frame.render_widget(
            HeaderTable {
                headers: &self.response.headers,
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
        let mut component = TestComponent::new(
            &harness,
            &terminal,
            ResponseBodyView::new(
                exchange.request.recipe_id.clone(),
                exchange.response,
            ),
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

        let exchange_id = response.id;
        let exchange = Exchange {
            response: response.into(),
            ..Exchange::factory(exchange_id)
        };
        let mut component = TestComponent::new(
            &harness,
            &terminal,
            ResponseBodyView::new(
                exchange.request.recipe_id.clone(),
                exchange.response,
            ),
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
