//! Test sending requests in the TUI. Important stuff!

use crate::common::{Runner, TestBackend, backend, collection_file};
use itertools::Itertools;
use rstest::rstest;
use slumber_core::{
    collection::{Collection, Recipe, RecipeId},
    database::{CollectionDatabase, ProfileFilter},
    http::HttpMethod,
    test_util::by_id,
};
use slumber_template::Template;
use slumber_tui::Tui;
use slumber_util::{DataDir, Factory, data_dir};
use std::sync::Arc;
use terminput::KeyCode;
use wiremock::{Mock, MockServer, ResponseTemplate, matchers};

mod common;

/// Send a request
#[rstest]
#[tokio::test]
async fn test_request(backend: TestBackend, data_dir: DataDir) {
    // Set up HTTP mock
    let server = MockServer::start().await;

    // Create a collection file to be loaded
    let collection = Collection {
        recipes: by_id([Recipe {
            id: "textBody".into(),
            url: url(&server, "/textBody"),
            method: HttpMethod::Post,
            body: Some("body data".into()),
            ..Recipe::factory(())
        }])
        .into(),
        ..Collection::factory(())
    };
    let collection_path = collection_file(&data_dir, &collection);
    // Rev up those fryers!!
    let tui = Tui::new(backend, Some(collection_path)).unwrap();

    // Run the request
    let mock_guard = mock_text_body().mount_as_scoped(&server).await;
    let tui = Runner::new(tui)
        .send_request(0)
        .wait_for_request(mock_guard)
        .await
        .done()
        .await;
    assert_persisted(tui.database(), &["textBody".into()]);
}

/// Resend a previous request
#[rstest]
#[tokio::test]
async fn test_resend(backend: TestBackend, data_dir: DataDir) {
    let server = MockServer::start().await;

    let collection = Collection {
        recipes: by_id([Recipe {
            id: "textBody".into(),
            url: url(&server, "/textBody"),
            method: HttpMethod::Post,
            body: Some("body data".into()),
            ..Recipe::factory(())
        }])
        .into(),
        ..Collection::factory(())
    };
    let collection_path = collection_file(&data_dir, &collection);
    let tui = Tui::new(backend, Some(collection_path)).unwrap();

    // Run the first request
    let mock_guard = mock_text_body().mount_as_scoped(&server).await;
    let tui = Runner::new(tui)
        .send_request(0)
        .wait_for_request(mock_guard)
        .await
        .done()
        .await;
    let exchange1 = tui
        .database()
        .get_latest_request(ProfileFilter::All, &"textBody".into())
        .unwrap()
        .expect("Request not in DB");
    assert_eq!(exchange1.request.body, b"body data".as_slice().into());

    // Resend that request
    let mock_guard = mock_text_body().mount_as_scoped(&server).await;
    let tui = Runner::new(tui)
        .send_key(KeyCode::Char('2')) // Select Request/Response pane
        .action(&[4]) // Resend Request action
        .wait_for_request(mock_guard)
        .await
        .done()
        .await;
    let exchange2 = tui
        .database()
        .get_latest_request(ProfileFilter::All, &"textBody".into())
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

/// Test persistence with triggered requests
///
/// - By default, triggered requests are persisted
/// - If disabled by recipe, it's not persisted
/// - If disabled globally, nothing is persisted
#[rstest]
#[case::enabled(true, true, &["downstream".into(), "upstream".into()])]
#[case::disabled_recipe(true, false, &["downstream".into()])]
#[case::disabled_global(false, true, &[])]
#[tokio::test]
async fn test_triggered_persisted(
    backend: TestBackend,
    data_dir: DataDir,
    #[case] config_persist: bool,
    #[case] upstream_persist: bool,
    #[case] expected: &[RecipeId],
) {
    use crate::common::config_file;
    use slumber_config::{Config, TuiConfig};

    let server = MockServer::start().await;
    // Mock both responses
    Mock::given(matchers::method("GET"))
        .and(matchers::path("/upstream"))
        .respond_with(ResponseTemplate::new(200).set_body_string("HELLO"))
        .mount(&server)
        .await;
    let mock_guard = Mock::given(matchers::method("POST"))
        .and(matchers::path("/downstream"))
        .and(matchers::body_string("HELLO"))
        .respond_with(ResponseTemplate::new(200).set_body_string("GOODBYE"))
        .mount_as_scoped(&server)
        .await;

    config_file(
        &data_dir,
        &Config {
            tui: TuiConfig {
                persist: config_persist,
                ..Default::default()
            },
            ..Default::default()
        },
    );
    let collection = Collection {
        recipes: by_id([
            Recipe {
                id: "upstream".into(),
                url: url(&server, "/upstream"),
                persist: upstream_persist,
                ..Recipe::factory(())
            },
            // A request that triggers another
            Recipe {
                id: "downstream".into(),
                url: url(&server, "/downstream"),
                method: HttpMethod::Post,
                body: Some(
                    "{{ response('upstream', trigger='always') }}".into(),
                ),
                ..Recipe::factory(())
            },
        ])
        .into(),
        ..Collection::factory(())
    };
    let collection_path = collection_file(&data_dir, &collection);
    let tui = Tui::new(backend, Some(collection_path)).unwrap();

    let tui = Runner::new(tui)
        .send_request(1) // Recipe: downstream
        .wait_for_request(mock_guard)
        .await
        .done()
        .await;
    assert_persisted(tui.database(), expected);
}

/// Requests are not persisted if disabled in recipe
#[rstest]
#[tokio::test]
async fn test_persisted_disabled(backend: TestBackend, data_dir: DataDir) {
    let server = MockServer::start().await;
    let mock_guard = Mock::given(matchers::method("GET"))
        .and(matchers::path("/notPersisted"))
        .respond_with(ResponseTemplate::new(200))
        .mount_as_scoped(&server)
        .await;

    let collection = Collection {
        recipes: by_id([Recipe {
            id: "notPersisted".into(),
            url: url(&server, "/notPersisted"),
            persist: false,
            ..Recipe::factory(())
        }])
        .into(),
        ..Collection::factory(())
    };
    let collection_path = collection_file(&data_dir, &collection);
    let tui = Tui::new(backend, Some(collection_path)).unwrap();

    let tui = Runner::new(tui)
        .send_request(0) // Recipe: notPersisted
        .wait_for_request(mock_guard)
        .await
        .done()
        .await;
    assert_persisted(tui.database(), &[]);
}

/// Build a mock for the POST endpoint
fn mock_text_body() -> Mock {
    Mock::given(matchers::method("POST"))
        .and(matchers::path("/textBody"))
        .respond_with(ResponseTemplate::new(200))
}

/// Build a URL on the mock server
fn url(server: &MockServer, path: &str) -> Template {
    format!("{host}{path}", host = server.uri())
        .parse()
        .unwrap()
}

/// Assert requests for a set of recipes were persisted in the expected order
///
/// Most recent requests are first.
#[track_caller]
fn assert_persisted(database: &CollectionDatabase, expected: &[RecipeId]) {
    let actual = database
        .get_all_requests()
        .unwrap()
        .into_iter()
        .map(|exchange| exchange.recipe_id)
        .collect_vec();
    assert_eq!(actual, expected);
}
