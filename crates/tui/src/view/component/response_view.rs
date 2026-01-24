//! Display for HTTP responses

use crate::{
    message::Message,
    view::{
        Component, ViewContext,
        common::header_table::HeaderTable,
        component::{
            Canvas, ComponentId, Draw, DrawMetadata, ToChild, internal::Child,
            queryable_body::QueryableBody,
        },
        context::UpdateContext,
        event::{Event, EventMatch},
        persistent::PersistentKey,
        util::view_text,
    },
};
use mime::Mime;
use serde::{Serialize, Serializer};
use slumber_config::Action;
use slumber_core::{collection::RecipeId, http::ResponseRecord};
use std::sync::Arc;

/// Display response body
#[derive(Debug)]
pub struct ResponseBodyView {
    id: ComponentId,
    response: Arc<ResponseRecord>,
    /// The presentable version of the response body, which may or may not
    /// match the response body. We apply transformations such as filter,
    /// prettification, or in the case of binary responses, a hex dump.
    body: QueryableBody<ResponseQueryKey>,
}

impl ResponseBodyView {
    pub fn new(recipe_id: RecipeId, response: Arc<ResponseRecord>) -> Self {
        // Select default query based on content type
        let config = &ViewContext::config().tui.commands;
        let mime = response.mime();
        let default_query = mime
            .as_ref()
            .and_then(|mime| config.default_query.get(mime).cloned());
        let body = QueryableBody::new(
            ResponseQueryKey { recipe_id, mime },
            Arc::clone(&response),
            default_query,
        );
        Self {
            id: ComponentId::default(),
            response,
            body,
        }
    }

    /// Open the visible body in the pager
    pub fn view_body(&self) {
        view_text(self.body.visible_text(), self.response.mime());
    }

    pub fn copy_body(&self) {
        // Use whatever text is visible to the user. This differs from saving
        // the body, because we can't copy binary content, so if the file is
        // binary we'll copy the hexcode text
        ViewContext::send_message(Message::CopyText(
            self.body.visible_text().to_string(),
        ));
    }

    pub fn save_response_body(&self) {
        // This will trigger a modal to ask the user for a path
        ViewContext::send_message(Message::SaveResponseBody {
            request_id: self.response.id,
            data: self.body.modified_text(),
        });
    }
}

impl Component for ResponseBodyView {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn update(&mut self, _: &mut UpdateContext, event: Event) -> EventMatch {
        event.m().action(|action, propagate| match action {
            Action::View => self.view_body(),
            _ => propagate.set(),
        })
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        vec![self.body.to_child_mut()]
    }
}

impl Draw for ResponseBodyView {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        canvas.draw(&self.body, (), metadata.area(), true);
    }
}

/// Persisted key for response body JSONPath query text box
#[derive(Debug, Serialize)]
struct ResponseQueryKey {
    recipe_id: RecipeId,
    /// Response queries are unique per-mime because query tools tend to be
    /// specific to content type. If the content type changes (e.g. JSON
    /// replaced by an HTML error page), we shouldn't keep applying JSON
    /// commands
    #[serde(serialize_with = "serialize_mime")]
    mime: Option<Mime>,
}

impl PersistentKey for ResponseQueryKey {
    type Value = String;
}

/// Serialize a MIME type as its string representation
#[expect(clippy::ref_option)]
fn serialize_mime<S>(
    mime: &Option<Mime>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    mime.as_ref().map(Mime::as_ref).serialize(serializer)
}

#[derive(Debug)]
pub struct ResponseHeadersView {
    id: ComponentId,
    response: Arc<ResponseRecord>,
}

impl ResponseHeadersView {
    pub fn new(response: Arc<ResponseRecord>) -> Self {
        Self {
            id: ComponentId::default(),
            response,
        }
    }
}

impl Component for ResponseHeadersView {
    fn id(&self) -> ComponentId {
        self.id
    }
}

impl Draw for ResponseHeadersView {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        canvas.render_widget(
            HeaderTable {
                headers: &self.response.headers,
            },
            metadata.area(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_util::{TestTerminal, terminal},
        view::test_util::{TestComponent, TestHarness, harness},
    };
    use indexmap::indexmap;
    use rstest::rstest;
    use slumber_core::{http::Exchange, test_util::header_map};
    use slumber_util::{Factory, assert_matches};
    use terminput::KeyCode;

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
        let component = TestComponent::new(
            &harness,
            &terminal,
            ResponseBodyView::new(
                exchange.request.recipe_id.clone(),
                exchange.response,
            ),
        );

        component.copy_body();
        let body = assert_matches!(
            harness.messages().pop_now(),
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
            // Type something into the query box
            component
                .int()
                .send_key(KeyCode::Char('/'))
                .send_text(query)
                .send_key(KeyCode::Enter)
                .assert()
                .empty();
            harness.run_task().await; // Run the command
            component.int().drain_draw().assert().empty();
        }

        component.save_response_body();
        let (request_id, data) = assert_matches!(
            harness.messages().pop_now(),
            Message::SaveResponseBody { request_id, data } => (request_id, data),
        );
        assert_eq!(request_id, exchange.id);
        assert_eq!(data.as_deref(), expected_body);
    }
}
