//! Test the `slumber generate` subcommand

mod common;

use rstest::rstest;
use serde_json::json;
use slumber_core::database::Database;
use wiremock::{Mock, MockServer, ResponseTemplate, matchers};

/// Test generating a curl command with different flags. Most of the request
/// components are tested in unit tests in the core crate, so we just need to
/// test CLI behavior here.
#[rstest]
#[case::url(
    &["getUser"],
    "curl -XGET --url 'http://server/users/username1'\n",
)]
#[case::profile(
    &["getUser", "-p", "profile2"],
    "curl -XGET --url 'http://server/users/username2'\n",
)]
#[case::overrides(
    &["getUser", "-o", "username=username3"],
    "curl -XGET --url 'http://server/users/username3'\n",
)]
fn test_generate_curl(
    #[case] arguments: &[&str],
    #[case] expected: &'static str,
) {
    let (mut command, _) = common::slumber();
    command.args(["generate", "curl"]);
    command.args(arguments);
    command.assert().success().stdout(expected);
}

/// Test failure when a downstream request is needed but cannot be triggered
#[test]
fn test_generate_curl_trigger_error() {
    let (mut command, _) = common::slumber();
    command.args(["generate", "curl", "chained"]);
    command.assert().failure().stderr(
        "Triggered requests are disabled by default; pass `--execute-triggers` to enable
  Rendering URL
  response()
  Triggering upstream recipe `getUser`
  Triggered request execution not allowed in this context
",
    );
}

/// Test upstream requests can be triggered with `--execute-triggers`
#[tokio::test]
async fn test_generate_curl_execute_trigger() {
    // Mock HTTP response
    let server = MockServer::start().await;
    let host = server.uri();
    let body = json!({"username": "username1"});
    Mock::given(matchers::method("GET"))
        .and(matchers::path("/users/username1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&body))
        .mount(&server)
        .await;

    let (mut command, data_dir) = common::slumber();
    command.args([
        "generate",
        "curl",
        "chained",
        "--execute-triggers",
        "-o",
        &format!("host={host}"),
    ]);
    command
        .assert()
        .success()
        .stdout(format!("curl -XGET --url '{host}/chained/username1'\n"));

    // Executed request should not have been persisted
    let database = Database::from_directory(&data_dir).unwrap();
    assert_eq!(&database.get_all_requests().unwrap(), &[]);
}

// More detailed test cases for curl are defined in unit tests
