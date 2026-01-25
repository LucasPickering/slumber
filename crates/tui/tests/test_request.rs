//! Test sending requests in the TUI. Important stuff!

use crate::common::{Runner, TestBackend, backend, collection_file};
use rstest::rstest;
use slumber_core::{
    collection::{Collection, Recipe, RecipeId},
    database::ProfileFilter,
    http::HttpMethod,
    test_util::by_id,
};
use slumber_tui::Tui;
use slumber_util::{DataDir, Factory, data_dir};
use std::sync::Arc;
use terminput::KeyCode;
use wiremock::{Mock, MockServer, ResponseTemplate, matchers};

mod common;

/// Resend a previous request
#[rstest]
#[tokio::test]
async fn test_resend(backend: TestBackend, data_dir: DataDir) {
    /// Build the HTTP mock. We have to make the same mock twice because we
    /// wait for it to complete after each test
    fn mock() -> Mock {
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/data"))
            .respond_with(ResponseTemplate::new(200))
    }

    // Set up HTTP mock
    let server = MockServer::start().await;
    let host = server.uri();

    // Create a collection file to be loaded
    let recipe_id: RecipeId = "textBody".into();
    let collection = Collection {
        recipes: by_id([Recipe {
            id: recipe_id.clone(),
            url: format!("{host}/data").parse().unwrap(),
            method: HttpMethod::Post,
            body: Some("body data".into()),
            ..Recipe::factory(())
        }])
        .into(),
        ..Collection::factory(())
    };
    let collection_path = collection_file(&data_dir, &collection);
    // Rev up those fryers!!
    let tui = Tui::new(backend.clone(), Some(collection_path)).unwrap();

    // Run the first request
    let mock_guard = mock().mount_as_scoped(&server).await;
    let tui = Runner::new(tui)
        .send_key(KeyCode::Enter) // Send the request
        .wait_for_request(mock_guard)
        .await
        .done()
        .await;
    let exchange1 = tui
        .database()
        .get_latest_request(ProfileFilter::All, &recipe_id)
        .unwrap()
        .expect("Request not in DB");
    assert_eq!(exchange1.request.body, b"body data".as_slice().into());

    // Resend that request
    let mock_guard = mock().mount_as_scoped(&server).await;
    let tui = Runner::new(tui)
        .send_key(KeyCode::Char('2')) // Select Request/Response pane
        .action(4) // Resend Request action
        .wait_for_request(mock_guard)
        .await
        .done()
        .await;
    let exchange2 = tui
        .database()
        .get_latest_request(ProfileFilter::All, &recipe_id)
        .unwrap()
        .expect("Request not in DB");

    // IDs are different, but the requests are identical
    assert_ne!(exchange1.id, exchange2.id);
    // To do the ID-agnostic comparison, we need to set the IDs to be identical.
    // That requires an owned request value
    let mut request2 = Arc::try_unwrap(exchange2.request).unwrap();
    request2.id = exchange1.id;
    assert_eq!(*exchange1.request, request2);
}
