//! Test the `slumber request` subcommand

mod common;

use indexmap::IndexMap;
use reqwest::StatusCode;
use rstest::rstest;
use serde::{Deserialize, Serialize};
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

/// Override various request components with `--override`
#[rstest]
#[case::url(
    &["--url", "{{host}}/override#test"],
    &[Assertion::Url("http://localhost/override#test")],
)]
#[case::query_single(
    // Only instance of a param
    &["--query", "foo=over"], &[Assertion::Query("foo", Some("over"))],
)]
#[case::query_many(
    // Multiple instances of param
    &["--query", "foo=over", "--query", "foo=over2"],
    &[Assertion::Query("foo", Some("over")), Assertion::Query("foo", Some("over2"))],
)]
#[case::query_additional(
    // Param not in the recipe
    &["--query", "add=over"], &[Assertion::Query("add", Some("over"))],
)]
#[case::query_omit(&["--query", "foo"], &[Assertion::Query("foo", None)])]
#[case::header(
    &["--header", "x-test=over"], &[Assertion::Header("x-test", Some("over"))],
)]
#[case::header_additional(
    &["--header", "x-new=over"], &[Assertion::Header("x-new", Some("over"))],
)]
#[case::header_omit(
    &["--header", "x-test"], &[Assertion::Header("x-test", None)],
)]
#[case::auth_basic(
    &["--basic", "user:hunter2"],
    &[Assertion::Header("authentication", Some("Basic dXNlcjpodW50ZXIy"))],
)]
#[case::auth_basic_alias(
    &["--user", "user:hunter2"],
    &[Assertion::Header("authentication", Some("Basic dXNlcjpodW50ZXIy"))],
)]
#[case::auth_token(
    &["--bearer", "my-token"],
    &[Assertion::Header("authentication", Some("Bearer my-token"))],
)]
#[case::auth_token_alias(
    &["--token", "my-token"],
    &[Assertion::Header("authentication", Some("Bearer my-token"))],
)]
#[case::form(&["--form", "foo=over"], &[Assertion::Form("foo", Some("over"))])]
#[case::form_additional(
    &["--form", "add=over"], &[Assertion::Form("add", Some("over"))],
)]
#[case::form_omit(&["--form", "foo"], &[Assertion::Form("foo", None)])]
#[case::body(&["--body", "over"], &[Assertion::Body("over")])]
#[case::body_alias(&["--data", "over"], &[Assertion::Body("over")])]
// TODO test JSON body override
#[case::profile(
    // First is a literal string, second is an int expression
    &["--override", "a=1", "-o", "b={{2}}"],
    &[Assertion::Body(r#"{"a":"1","b":2}"#)],
)]
#[tokio::test]
async fn test_request_override(
    #[case] args: &[&str],
    #[case] assertions: &[Assertion],
) {
    /// Echo response
    #[derive(Serialize, Deserialize)]
    struct Out {
        url: String,
        query: IndexMap<String, String>,
        headers: IndexMap<String, String>,
        form: IndexMap<String, String>,
        body: String,
    }

    assert_ne!(assertions, &[], "assertions cannot be empty");

    // Mock HTTP response
    let server = MockServer::start().await;
    let host = server.uri();
    Mock::given(matchers::method("POST"))
        .and(matchers::path("/override"))
        .respond_with(|req: &Request| {
            // Send a response that describes the request
            // Drop query params because they're elsewhere
            let mut url = req.url.clone();
            url.query_pairs_mut().clear();
            let query = req
                .url
                .query_pairs()
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect();
            let headers = req
                .headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_str().unwrap().to_owned()))
                .collect();
            let form = IndexMap::new(); // TODO
            let body = String::from_utf8(req.body.clone()).unwrap();
            let response = Out {
                url: url.to_string(),
                query,
                headers,
                form,
                body,
            };
            ResponseTemplate::new(200).set_body_json(&response)
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

    let r: Out = serde_json::from_slice(&assert.get_output().stdout).unwrap();
    // Apply each assertion
    for assertion in assertions {
        match assertion {
            Assertion::Url(url) => assert_eq!(&r.url, url, "url"),
            Assertion::Query(param, value) => {
                assert_eq!(
                    r.query.get(*param).map(String::as_str),
                    *value,
                    "query parameter `{param}`"
                );
            }
            Assertion::Header(header, value) => {
                assert_eq!(
                    r.headers.get(*header).map(String::as_str),
                    *value,
                    "header `{header}`"
                );
            }
            Assertion::Form(field, value) => {
                assert_eq!(
                    r.form.get(*field).map(String::as_str),
                    *value,
                    "form field `{field}`"
                );
            }
            Assertion::Body(body) => assert_eq!(&r.body, body, "body text"),
        }
    }
}

/// Helper to define what part of a request we're asserting
#[derive(Debug, PartialEq)]
enum Assertion {
    /// Assert URL matches a value
    Url(&'static str),
    /// Assert a query parameter matches a value. `None` asserts the parameter
    /// is missing. If there are multiple values for the parameter, assert
    /// that any one of them matches the value.
    Query(&'static str, Option<&'static str>),
    /// Assert a header matches a value. `None` asserts the header is
    /// missing
    Header(&'static str, Option<&'static str>),
    /// Assert a body form field matches a value. `None` asserts the field is
    /// missing
    Form(&'static str, Option<&'static str>),
    /// Assert body text matches a value
    Body(&'static str),
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
