//! Test the `slumber request` subcommand

mod common;

use reqwest::StatusCode;
use serde_json::json;
use slumber_core::{db::Database, http::ExchangeSummary};
use slumber_util::assert_matches;
use wiremock::{Mock, MockServer, ResponseTemplate, matchers};

/// Test the basic request use case, including `--profile` and `--override`
#[tokio::test]
async fn test_request() {
    // Mock HTTP response
    let server = MockServer::start().await;
    let host = server.uri();
    let body = json!({
        "username": "username2",
        "name": "Frederick Smidgen"
    });
    Mock::given(matchers::method("POST"))
        .and(matchers::path("/json"))
        .and(matchers::body_json(&body))
        .respond_with(ResponseTemplate::new(200).set_body_json(&body))
        .mount(&server)
        .await;

    let (mut command, data_dir) = common::slumber();
    command.args([
        "request",
        "jsonBody",
        "--profile",
        "profile2",
        "-o",
        &format!("host={host}"),
    ]);
    command.assert().success().stdout(body.to_string());

    // Requests are not persisted
    let database = Database::from_directory(&data_dir).unwrap();
    assert_eq!(
        &database.get_all_requests().unwrap(),
        &[],
        "Expected request to not be persisted"
    );
}

/// Test the `--verbose` flag
#[tokio::test]
async fn test_request_verbose() {
    // Mock HTTP response
    let server = MockServer::start().await;
    let host = server.uri();
    let body = json!({
        "username": "username1",
        "name": "Frederick Smidgen"
    });
    Mock::given(matchers::method("POST"))
        .and(matchers::path("/json"))
        .and(matchers::body_json(&body))
        .respond_with(ResponseTemplate::new(200).set_body_json(&body))
        .mount(&server)
        .await;

    let (mut command, _) = common::slumber();
    command.args([
        "request",
        "jsonBody",
        "--verbose",
        "-o",
        &format!("host={host}"),
    ]);
    command.assert().success().stdout(body.to_string());
}

/// Test the `--dry-run` flag
#[tokio::test]
async fn test_request_dry_run() {
    let (mut command, _) = common::slumber();
    command.args(["request", "jsonBody", "--dry-run"]);
    command.assert().success().stderr(
        "> POST http://server/json HTTP/1.1
> content-type: application/json
> {
  \"username\": \"username1\",
  \"name\": \"Frederick Smidgen\"
}
",
    );
}

/// Test the `--exit-status` flag
#[tokio::test]
async fn test_request_exit_status() {
    let server = MockServer::start().await;
    let host = server.uri();
    let body = json!({
        "username": "username1",
        "name": "Frederick Smidgen"
    });
    Mock::given(matchers::method("POST"))
        .and(matchers::path("/json"))
        .and(matchers::body_json(&body))
        .respond_with(ResponseTemplate::new(400).set_body_json(&body))
        .mount(&server)
        .await;

    let (mut command, _) = common::slumber();
    command.args([
        "request",
        "jsonBody",
        "--exit-status",
        "-o",
        &format!("host={host}"),
    ]);
    command.assert().failure().stdout(body.to_string());
}

/// Test the `--persist` flag
#[tokio::test]
async fn test_request_persist() {
    // Mock HTTP response
    let server = MockServer::start().await;
    let host = server.uri();
    let body = json!({
        "username": "username1",
        "name": "Frederick Smidgen"
    });
    Mock::given(matchers::method("POST"))
        .and(matchers::path("/json"))
        .and(matchers::body_json(&body))
        .respond_with(ResponseTemplate::new(200).set_body_json(&body))
        .mount(&server)
        .await;

    let (mut command, data_dir) = common::slumber();
    command.args([
        "request",
        "jsonBody",
        "--persist",
        "-o",
        &format!("host={host}"),
    ]);
    command.assert().success().stdout(body.to_string());

    // Request was persisted
    let database = Database::from_directory(&data_dir).unwrap();
    let requests = database.get_all_requests().unwrap();
    let (recipe_id, profile_id, status) = assert_matches!(
        requests.as_slice(),
        [ExchangeSummary {
            recipe_id,
            profile_id,
            status,
            ..
        }] => (recipe_id, profile_id, status),
    );
    assert_eq!(recipe_id, &"jsonBody".into());
    assert_eq!(profile_id, &Some("profile1".into()));
    assert_eq!(*status, StatusCode::OK);
}
