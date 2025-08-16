//! Tests for template profiles and functions. We don't need to test template
//! syntax/parsing here because that's handled in the template lib.

use crate::{
    collection::{Profile, Recipe},
    database::CollectionDatabase,
    http::{Exchange, HttpEngine, RequestId, RequestRecord, ResponseRecord},
    render::TemplateContext,
    test_util::{
        TestHttpProvider, TestPrompter, TestSelectPrompter, by_id, header_map,
        http_engine,
    },
};
use chrono::{DateTime, Utc};
use indexmap::{IndexMap, indexmap};
use rstest::rstest;
use slumber_template::{Expression, Template};
use slumber_util::{Factory, TempDir, assert_result, temp_dir};
use std::time::Duration;
use tokio::fs;
use wiremock::{Mock, MockServer, ResponseTemplate, matchers};

/// Profile fields
#[tokio::test]
async fn test_profile() {
    let template: Template = "{{ host }}/users/{{ user_id }}".parse().unwrap();

    // Put some profile data in the context
    let profile_data = indexmap! {
        "host".into() => "http://localhost".into(),
        "user_id".into() => "1".into(),
    };
    let profile = Profile {
        data: profile_data,
        ..Profile::factory(())
    };
    let context = TemplateContext::factory((by_id([profile]), IndexMap::new()));

    assert_eq!(
        template.render_bytes(&context).await.unwrap(),
        "http://localhost/users/1"
    );
}

/// Override profiles fields
#[tokio::test]
async fn test_override() {
    let template: Template = "{{ host }}/users/{{ user_id }}".parse().unwrap();

    // Put some profile data and overrides in the context
    let profile_data = indexmap! {
        "host".into() => "http://localhost".into(),
        "user_id".into() => "1".into(),
    };
    let profile = Profile {
        data: profile_data,
        ..Profile::factory(())
    };
    let context = TemplateContext {
        overrides: indexmap! {"host".into() => "http://override".into()},
        ..TemplateContext::factory((by_id([profile]), IndexMap::new()))
    };

    assert_eq!(
        template.render_bytes(&context).await.unwrap(),
        "http://override/users/1"
    );
}

/// `command()`
#[rstest]
#[case::no_stdin(vec!["echo", "test"], None, Ok(b"test\n" as _))]
#[case::stdin(vec!["cat", "-"], Some(b"test" as _), Ok(b"test" as _))]
#[case::binary_output(vec!["cat", "-"], Some(invalid_utf8()), Ok(invalid_utf8()))]
#[case::error_empty(vec![], None, Err("Command must have at least one element"))]
#[case::error_bad_command(vec!["fake"], None, Err("Executing command `fake`"))]
#[tokio::test]
async fn test_command(
    #[case] command: Vec<&str>,
    #[case] stdin: Option<&'static [u8]>,
    #[case] expected: Result<&[u8], &str>,
) {
    let template = Template::function_call(
        "command",
        [command.into_iter().map(Expression::from).collect()],
        [("stdin", stdin.map(Expression::from))],
    );
    assert_result(
        template.render_bytes(&TemplateContext::factory(())).await,
        expected,
    );
}

/// `concat()`
#[rstest]
#[case::empty(vec![], Ok(""))]
#[case::values(
    vec!["data/".into(), "file".into(), ".json".into()],
    Ok("data/file.json"),
)]
#[case::error_binary(vec![b"\xc3\x28".into()], Err("invalid utf-8 sequence"))]
#[tokio::test]
async fn test_concat(
    #[case] elements: Vec<Expression>,
    #[case] expected: Result<&str, &str>,
) {
    let template = Template::function_call("concat", [elements.into()], []);
    assert_result(
        template.render_string(&TemplateContext::factory(())).await,
        expected,
    );
}

/// `env()`
#[rstest]
#[case::set("CARGO_PKG_NAME", Ok("slumber_core"))]
#[case::unset("NOT_A_REAL_ENV_VAR", Ok(""))]
#[tokio::test]
async fn test_env(
    #[case] variable: &str,
    #[case] expected: Result<&str, &str>,
) {
    let template = Template::function_call("env", [variable.into()], []);
    assert_result(
        template.render_bytes(&TemplateContext::factory(())).await,
        expected,
    );
}

/// `file()`
#[rstest]
#[case::text("data.txt", Ok(b"text" as _))]
#[case::binary("data.bin", Ok(invalid_utf8()))]
#[case::error_not_exists(
    "fake.txt",
    Err(if cfg!(unix) {
        "No such file or directory"
    } else {
        "The system cannot find the file specified"
    })
)]
#[tokio::test]
async fn test_file(
    temp_dir: TempDir,
    #[case] path: &str,
    #[case] expected: Result<&[u8], &str>,
) {
    // Create two test files
    fs::write(temp_dir.join("data.txt"), "text").await.unwrap();
    fs::write(temp_dir.join("data.bin"), invalid_utf8())
        .await
        .unwrap();

    let template = Template::function_call(
        "file",
        [temp_dir
            .join(path)
            .into_os_string()
            .into_string()
            .unwrap()
            .into()],
        [],
    );
    assert_result(
        template.render_bytes(&TemplateContext::factory(())).await,
        expected,
    );
}

/// `jsonpath()`
#[rstest]
// Default mode is auto
#[case::mode_default_one("$[1]", None, None, Ok("b"))]
#[case::mode_default_many("$[*]", None, None, Ok("['a', 'b', 'c']"))]
#[case::error_auto_empty(
    "$[5]",
    Some("auto"),
    None,
    Err("No results from JSONPath query")
)]
#[case::mode_auto_one("$[1]", Some("auto"), None, Ok("b"))]
#[case::mode_auto_many("$[*]", Some("auto"), None, Ok("['a', 'b', 'c']"))]
#[case::mode_array_zero("$[5]", Some("array"), None, Ok("[]"))]
#[case::mode_array_one("$[1]", Some("array"), None, Ok("['b']"))]
#[case::mode_array_many("$[*]", Some("array"), None, Ok("['a', 'b', 'c']"))]
#[case::error_single_empty(
    "$[5]",
    Some("single"),
    None,
    Err("Expected exactly one result")
)]
#[case::mode_single_one("$[1]", Some("single"), None, Ok("b"))]
#[case::error_single_many(
    "$[*]",
    Some("single"),
    None,
    Err("Expected exactly one result")
)]
// Binary content can't be converted to JSON
#[case::error_binary(
    "$[1]",
    None,
    Some(invalid_utf8().into()),
    Err("Error parsing bytes as JSON")
)]
#[tokio::test]
async fn test_jsonpath(
    #[case] query: &str,
    #[case] mode: Option<&str>,
    #[case] json: Option<Expression>, // If not given, use a default
    #[case] expected: Result<&str, &str>,
) {
    let json: Expression = json.unwrap_or_else(|| {
        vec!["a", "b", "c"]
            .into_iter()
            .map(Expression::from)
            .collect()
    });
    let template = Template::function_call(
        "jsonpath",
        [json, query.into()],
        [("mode", mode.map(Expression::from))],
    );
    assert_result(
        template.render_string(&TemplateContext::factory(())).await,
        expected,
    );
}

/// `prompt()`
#[rstest]
#[case::reply(Some("test"), None, false, Ok("test"))]
#[case::default(None, Some("default"), false, Ok("default"))]
#[case::sensitive(Some("test"), None, true, Ok("••••"))]
#[case::error_no_reply(None, None, false, Err("No reply"))]
#[tokio::test]
async fn test_prompt(
    #[case] reply: Option<&str>,
    #[case] default: Option<&str>,
    #[case] sensitive: bool,
    #[case] expected: Result<&str, &str>,
) {
    let template = Template::function_call(
        "prompt",
        [],
        [
            // We don't have a good way to test that message is forwarded but
            // we can at least make sure it's a real arg
            ("message", Some("Enter something please!!".into())),
            ("default", default.map(Expression::from)),
            ("sensitive", Some(sensitive.into())),
        ],
    );
    let context = TemplateContext {
        prompter: Box::new(TestPrompter::new(reply.into_iter())),
        show_sensitive: false,
        ..TemplateContext::factory(())
    };
    assert_result(template.render_bytes(&context).await, expected);
}

/// `response()`
#[rstest]
// ===== Response is cached =====
#[case::cached_default("upstream", None, Some(Utc::now()), false, Ok("cached"))]
#[case::cached_never(
    "upstream",
    Some("never"),
    Some(Utc::now()),
    false,
    Ok("cached")
)]
#[case::cached_no_history(
    "upstream",
    Some("no_history"),
    Some(Utc::now()),
    false,
    Ok("cached")
)]
#[case::cached_expire_duration(
    // There's something in history and it's valid
    "upstream",
    Some("30m"),
    Some(Utc::now()),
    false,
    Ok("cached")
)]
// ===== Request will be triggered =====
#[case::trigger_no_history(
    "upstream",
    Some("no_history"),
    None,
    true,
    Ok("triggered")
)]
#[case::trigger_expire_empty(
    "upstream",
    Some("0s"),
    None,
    true,
    Ok("triggered")
)] // Nothing in history
#[case::trigger_expire_duration(
    // There's something in history but it's expired
    "upstream",
    Some("60s"),
    Some(Utc::now() - Duration::from_secs(100)),
    true,
    Ok("triggered")
)]
#[case::trigger_always_no_history(
    "upstream",
    Some("always"),
    None,
    true,
    Ok("triggered")
)]
#[case::trigger_always_with_history(
    "upstream",
    Some("always"),
    Some(Utc::now()),
    true,
    Ok("triggered")
)]
// ===== Error =====
#[case::error_unknown_recipe(
    "fake",
    Some("never"),
    None,
    true,
    Err("Unknown recipe `fake`")
)]
#[case::error_no_response(
    // Recipe exists but has no history in the DB
    "upstream",
    Some("never"),
    None,
    true,
    Err("No response available"),
)]
#[case::error_trigger_disabled(
    // Upstream can't be executed because triggers are disabled
    "upstream",
    Some("always"),
    None,
    false,
    Err("Triggered request execution not allowed in this context"),
)]
#[case::error_upstream(
    // Error making the request
    "upstream_error",
    Some("always"),
    None,
    true,
    Err("Triggering upstream recipe `upstream_error`")
)]
#[tokio::test]
async fn test_response(
    #[case] recipe: &str,
    #[case] trigger: Option<&str>,
    // Should there be a cached response, and if so, when did it happen?
    #[case] history_time: Option<DateTime<Utc>>,
    #[case] trigger_enabled: bool,
    #[case] expected: Result<&str, &str>,
    http_engine: HttpEngine,
) {
    let template = Template::function_call(
        "response",
        [recipe.into()],
        [("trigger", trigger.map(Expression::from))],
    );

    // Mock HTTP response
    let server = MockServer::start().await;
    let host = server.uri();
    // /get -> 200
    Mock::given(matchers::method("GET"))
        .and(matchers::path("/get"))
        .respond_with(ResponseTemplate::new(200).set_body_string("triggered"))
        .mount(&server)
        .await;

    // Create context
    let recipes = [
        Recipe {
            id: "upstream".into(),
            url: format!("{host}/get").into(),
            ..Recipe::factory(())
        },
        Recipe {
            // This has no response
            id: "upstream_error".into(),
            url: "not a real url".into(),
            ..Recipe::factory(())
        },
    ];
    // If an exchange was given, include it in the DB
    let database = CollectionDatabase::factory(());
    if let Some(history_time) = history_time {
        let id = RequestId::new();
        let exchange = Exchange {
            id,
            request: RequestRecord::factory((id, None, recipes[0].id.clone()))
                .into(),
            response: ResponseRecord {
                body: "cached".into(),
                ..ResponseRecord::factory(id)
            }
            .into(),
            // Set the timestamps so we can test the expire trigger
            start_time: history_time,
            end_time: history_time,
        };
        database.insert_exchange(&exchange).unwrap();
    }
    let context = TemplateContext {
        // Provide the HTTP engine if triggers are enabled
        http_provider: Box::new(TestHttpProvider::new(
            database,
            trigger_enabled.then_some(http_engine),
        )),
        ..TemplateContext::factory((IndexMap::new(), by_id(recipes)))
    };

    assert_result(template.render_bytes(&context).await, expected);
}

/// `response_header()`. We're leaning on the `response()` tests for most of
/// the work here, and just testing things specific to headers
#[rstest]
#[case::cached("My-Header", None, Ok("cached"))]
#[case::triggered("My-Header", Some("1m"), Ok("triggered"))]
#[case::error_missing_header(
    "Unknown",
    None,
    Err("Header `Unknown` not in response")
)]
#[tokio::test]
async fn test_response_header(
    #[case] header: &str,
    #[case] trigger: Option<&str>,
    #[case] expected: Result<&str, &str>,
    http_engine: HttpEngine,
) {
    let template = Template::function_call(
        "response_header",
        ["upstream".into(), header.into()],
        [("trigger", trigger.map(Expression::from))],
    );

    // Mock HTTP response
    let server = MockServer::start().await;
    let host = server.uri();
    Mock::given(matchers::method("GET"))
        .and(matchers::path("/get"))
        .respond_with(
            ResponseTemplate::new(200).insert_header("My-Header", "triggered"),
        )
        .mount(&server)
        .await;

    // Create context
    let recipe = Recipe {
        id: "upstream".into(),
        url: format!("{host}/get").into(),
        ..Recipe::factory(())
    };
    // If an exchange was requested, include it in the DB
    let database = CollectionDatabase::factory(());
    let id = RequestId::new();
    // Create a response that's 1hr old
    let exchange = Exchange {
        response: ResponseRecord {
            headers: header_map([("My-Header", "cached")]),
            ..ResponseRecord::factory(id)
        }
        .into(),
        end_time: Utc::now() - Duration::from_secs(60 * 60),
        ..Exchange::factory((id, None, recipe.id.clone()))
    };
    database.insert_exchange(&exchange).unwrap();
    let context = TemplateContext {
        // Provide the HTTP engine if triggers are enabled
        http_provider: Box::new(TestHttpProvider::new(
            database,
            Some(http_engine),
        )),
        ..TemplateContext::factory((IndexMap::new(), by_id([recipe])))
    };

    assert_result(template.render_bytes(&context).await, expected);
}

/// `select()`
#[rstest]
#[case::reply(vec!["first", "second"], Some(1), Ok("second"))]
#[case::empty(vec![], None, Err("Select has no options"))]
#[case::no_reply(vec!["test"], None, Err("No reply"))]
#[tokio::test]
async fn test_select(
    #[case] options: Vec<&str>,
    #[case] select: Option<usize>,
    #[case] expected: Result<&str, &str>,
) {
    let template = Template::function_call(
        "select",
        [options.into_iter().map(Expression::from).collect()],
        [],
    );
    let context = TemplateContext {
        prompter: Box::new(TestSelectPrompter::new(select.into_iter())),
        ..TemplateContext::factory(())
    };
    assert_result(template.render_bytes(&context).await, expected);
}

/// `sensitive()`
#[rstest]
#[case::masked("test", "••••")]
#[tokio::test]
async fn test_sensitive(#[case] input: &str, #[case] expected: &str) {
    let template = Template::function_call("sensitive", [input.into()], []);
    let context = TemplateContext {
        show_sensitive: false,
        ..TemplateContext::factory(())
    };
    assert_eq!(template.render_bytes(&context).await.unwrap(), expected);
}

/// `trim()`
#[rstest]
#[case::default("  test  ", None, "test")]
#[case::start("  test  ", Some("start"), "test  ")]
#[case::end("  test  ", Some("end"), "  test")]
#[case::both("  test  ", Some("both"), "test")]
#[tokio::test]
async fn test_trim(
    #[case] input: &str,
    #[case] mode: Option<&str>,
    #[case] expected: &str,
) {
    let template = Template::function_call(
        "trim",
        [input.into()],
        [("mode", mode.map(Expression::from))],
    );
    assert_eq!(
        template
            .render_bytes(&TemplateContext::factory(()))
            .await
            .unwrap(),
        expected
    );
}

/// Bytes that can't be converted to a string
fn invalid_utf8() -> &'static [u8] {
    b"\xc3\x28"
}
