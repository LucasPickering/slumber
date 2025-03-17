//! Test the `slumber generate` subcommand

use serde_json::json;
use wiremock::{Mock, MockServer, ResponseTemplate, matchers};

mod common;

/// Test generating a curl command with:
/// - URL
/// - Query params
/// - Headers
#[test]
fn test_generate_curl() {
    let (mut command, _) = common::slumber();
    command.args(["generate", "curl", "getUser"]);
    command
        .assert()
        .success()
        .stdout("curl -XGET --url 'http://server/users/username1'\n");
}

/// Make sure the profile option is reflected correctly
#[test]
fn test_generate_curl_profile() {
    let (mut command, _) = common::slumber();
    command.args(["generate", "curl", "getUser", "-p", "profile2"]);
    command
        .assert()
        .success()
        .stdout("curl -XGET --url 'http://server/users/username2'\n");
}

/// Make sure field overrides are applied correctly
#[test]
fn test_generate_curl_override() {
    let (mut command, _) = common::slumber();
    command.args(["generate", "curl", "getUser", "-o", "username=username3"]);
    command
        .assert()
        .success()
        .stdout("curl -XGET --url 'http://server/users/username3'\n");
}

/// Test failure when a downstream request is needed but cannot be triggered
#[test]
fn test_generate_curl_trigger_error() {
    let (mut command, _) = common::slumber();
    command.args(["generate", "curl", "chained"]);
    command.assert().failure().stderr(
        "Triggered requests are disabled by default; pass `--execute-triggers` to enable
  Error rendering URL
  Resolving chain `trigger`
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

    let (mut command, _) = common::slumber();
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
}

// More detailed test cases for curl are defined in unit tests
