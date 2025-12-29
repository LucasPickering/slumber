//! Test the `slumber request` subcommand

mod common;

use indexmap::IndexMap;
use predicates::prelude::predicate;
use reqwest::StatusCode;
use rstest::rstest;
use serde_json::json;
use slumber_core::{database::Database, http::ExchangeSummary};
use slumber_util::assert_matches;
use wiremock::{Mock, MockServer, Request, ResponseTemplate, matchers};

/// Test the basic request use case, including `--profile`
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
    command.args(["request", "jsonBody", "--profile", "profile2"]);
    command
        .env("HOST", host)
        .assert()
        .success()
        .stdout(body.to_string());

    // CLI requests are **not** persisted
    let database = Database::from_directory(&data_dir).unwrap();
    assert_eq!(
        &database.get_all_requests().unwrap(),
        &[],
        "Expected request to not be persisted"
    );
}

/// Override profile field with `--override`
#[rstest]
#[case::overwrite(
    &["--override", "a=1", "--override", "b=2"], r#"{"a":"1","b":"2"}"#
)]
#[case::alias(&["-o", "a=1"], r#"{"a":"1","b":"0"}"#)]
#[tokio::test]
async fn test_request_override_profile(
    #[case] args: &[&str],
    #[case] expected_body: &'static str,
) {
    // Mock HTTP response
    let server = MockServer::start().await;
    let host = server.uri();
    Mock::given(matchers::method("POST"))
        .and(matchers::path("/override"))
        // Echo the body
        .respond_with(|req: &Request| {
            ResponseTemplate::new(200).set_body_bytes(req.body.clone())
        })
        .mount(&server)
        .await;

    let (mut command, _) = common::slumber();
    command
        .args(["request", "override", "--exit-status"])
        .args(args)
        .env("HOST", host)
        .assert()
        .success()
        .stdout(expected_body);
}

/// Override headers with `--header`
#[rstest]
#[case::overwrite(&["--header", "x-test=over"], &[("x-test", Some("over"))])]
#[case::alias(&["-H", "x-test=over"], &[("x-test", Some("over"))])]
#[case::additional(&["--header", "x-new=over"], &[("x-new", Some("over"))])]
#[case::omit(&["--header", "x-test"], &[("x-test", None)])]
#[tokio::test]
async fn test_request_override_headers(
    #[case] args: &[&str],
    #[case] expected_headers: &[(&str, Option<&str>)],
) {
    type Headers<'a> = IndexMap<&'a str, &'a str>;

    // Mock HTTP response
    let server = MockServer::start().await;
    let host = server.uri();
    Mock::given(matchers::method("POST"))
        .and(matchers::path("/override"))
        .respond_with(|req: &Request| {
            let headers: Headers = req
                .headers
                .iter()
                .map(|(k, v)| (k.as_str(), v.to_str().unwrap()))
                .collect();
            ResponseTemplate::new(200).set_body_json(&headers)
        })
        .mount(&server)
        .await;

    let (mut command, _) = common::slumber();
    let assert = command
        .args(["request", "override", "--exit-status"])
        .args(args)
        .env("HOST", host)
        .assert()
        .success();

    let actual: Headers =
        serde_json::from_slice(&assert.get_output().stdout).unwrap();
    // Only assert headers that are included in the expectation. Otherwise we'd
    // have to list all the generic ones, like Content-Type
    for (header, expected) in expected_headers {
        assert_eq!(actual.get(header).copied(), *expected, "header `{header}`");
    }
}

/// Override authentication with `--basic` and `--bearer`
#[rstest]
#[case::basic(&["--basic", "user:hunter2"], "Basic dXNlcjpodW50ZXIy")]
#[case::basic_alias(&["--user", "user:hunter2"], "Basic dXNlcjpodW50ZXIy")]
#[case::token(&["--bearer", "my-token"], "Bearer my-token")]
#[case::token_alias(&["--token", "my-token"], "Bearer my-token")]
#[tokio::test]
async fn test_request_override_auth(
    #[case] args: &[&str],
    #[case] expected_auth: &'static str,
) {
    // Mock HTTP response
    let server = MockServer::start().await;
    let host = server.uri();
    Mock::given(matchers::method("POST"))
        .and(matchers::path("/override"))
        .respond_with(|req: &Request| {
            let auth = req
                .headers
                .get("Authorization")
                .map(|value| value.to_str().unwrap())
                .unwrap_or_default();
            ResponseTemplate::new(200).set_body_string(auth)
        })
        .mount(&server)
        .await;

    let (mut command, _) = common::slumber();
    command
        .args(["request", "override", "--exit-status"])
        .args(args)
        .env("HOST", host)
        .assert()
        .success()
        .stdout(expected_auth);
}

/// Error cases in auth override
#[rstest]
#[case::mutually_exclusive(
        &["--basic", "user:pass", "--bearer", "token"],
        "the argument '--basic <username:password>' cannot be used with \
        '--bearer <token>'",
)]
#[case::invalid_template(&["--bearer", "{{unclosed"], "invalid expression")]
// I couldn't figure out how to send input to the CLI, so we test that it
// prompts by ensuring the prompt fails.
#[case::basic_prompt(&["--basic", "user"], "No reply")]
#[tokio::test]
async fn test_request_override_auth_error(
    #[case] args: &[&str],
    #[case] expected_error: &'static str,
) {
    let (mut command, _) = common::slumber();
    command
        .args(["request", "override", "--exit-status"])
        .args(args)
        .assert()
        .failure()
        .stderr(predicate::str::contains(expected_error));
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
> {\"username\":\"username1\",\"name\":\"Frederick Smidgen\"}
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

/// Test the `--persist` flag. The main request should be persisted, but the
/// triggered will **will not**. This is partially a technical decision (makes
/// the code simpler) and partially a user-friendliness one. It's not entirely
/// clear which one the user would wait, so prefer the less "destructive"
/// option.
#[tokio::test]
async fn test_request_persist() {
    // Mock HTTP response
    let server = MockServer::start().await;
    let host = server.uri();
    let body = json!({
        "username": "username1",
        "name": "Frederick Smidgen"
    });
    Mock::given(matchers::method("GET"))
        .and(matchers::path("/users/username1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&body))
        .mount(&server)
        .await;
    Mock::given(matchers::method("GET"))
        .and(matchers::path("/chained/username1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&body))
        .mount(&server)
        .await;

    let (mut command, data_dir) = common::slumber();
    command.args([
        "request",
        "chained",
        "--persist",
        "-o",
        &format!("host={host}"),
    ]);
    command.assert().success().stdout(body.to_string());

    // Main request was persisted, triggered request was _not_
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
    assert_eq!(recipe_id, &"chained".into());
    assert_eq!(profile_id, &Some("profile1".into()));
    assert_eq!(*status, StatusCode::OK);
}

/// When loading a collection, the DB should be updated to reflect its name
#[tokio::test]
async fn test_set_collection_name() {
    let (mut command, data_dir) = common::slumber();

    // Sanity check: no collections in the DB
    let database = Database::from_directory(&data_dir).unwrap();
    assert_eq!(database.get_collections().unwrap().as_slice(), &[]);

    command.args(["request", "jsonBody", "--dry-run"]);
    command.assert().success();

    // Collection name was updated in the DB
    assert_eq!(
        database.get_collections().unwrap()[0].name.as_deref(),
        Some("CLI Tests")
    );
}
