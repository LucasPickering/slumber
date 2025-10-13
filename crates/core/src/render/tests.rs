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
use bytes::BytesMut;
use chrono::{DateTime, Utc};
use futures::{StreamExt, TryStreamExt};
use indexmap::{IndexMap, indexmap};
use rstest::rstest;
use serde_json::json;
use slumber_template::{Expression, Literal, StreamSource, Template, Value};
use slumber_util::{
    Factory, TempDir, assert_matches, assert_result, paths::get_repo_root,
    temp_dir,
};
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
        template
            .render_bytes(&context.streaming(false))
            .await
            .unwrap(),
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
        template
            .render_bytes(&context.streaming(false))
            .await
            .unwrap(),
        "http://override/users/1"
    );
}

/// `base64()`
#[rstest]
#[case::encode_string(b"test", false, Ok("dGVzdA==".as_bytes()))]
#[case::encode_bytes(invalid_utf8(), false, Ok("wyg=".as_bytes()))]
#[case::decode_string(b"dGVzdA==", true, Ok("test".as_bytes()))]
#[case::decode_bytes(b"wyg=", true, Ok(invalid_utf8()))]
#[case::error_invalid_base64(b"not base64", true, Err("Invalid symbol"))]
#[tokio::test]
async fn test_base64(
    #[case] input: &'static [u8],
    #[case] decode: bool,
    #[case] expected: Result<&'static [u8], &str>,
) {
    let template = Template::function_call(
        "base64",
        [input.into()],
        [("decode", Some(decode.into()))],
    );
    assert_result(
        template
            .render_bytes(&TemplateContext::factory(()).streaming(false))
            .await,
        expected,
    );
}

/// `boolean()`
#[rstest]
#[case::null(Expression::Literal(Literal::Null), false)]
#[case::bool_false(false.into(), false)]
#[case::bool_true(true.into(), true)]
#[case::float_zero(0.0.into(), false)]
#[case::float_one(1.0.into(), true)]
#[case::int_zero(0.into(), false)]
#[case::int_one(1.into(), true)]
#[case::string_empty("".into(), false)]
#[case::string_zero("0".into(), true)]
#[case::bytes_empty(b"".into(), false)]
#[case::bytes_invalid(invalid_utf8().into(), true)]
#[case::array_empty(vec![].into(), false)]
#[case::array_filled(vec!["1".into(), "2".into()].into(), true)]
#[tokio::test]
async fn test_boolean(#[case] input: Expression, #[case] expected: bool) {
    let template = Template::function_call("boolean", [input], []);
    assert_result(
        template
            .render(&TemplateContext::factory(()).streaming(false))
            .await
            .try_collect_value()
            .await,
        Ok(Value::Boolean(expected)),
    );
}

/// `command()`
#[rstest]
#[case::basic(vec!["echo", "test"], None, None, Ok("test\n".as_bytes()))]
// The command and output is platform-specific, and it's annoying to test both
// Unix and Windows. Since we don't have any platform-specific logic in our own
// code, there isn't much value in testing all platforms.
#[cfg_attr(
    unix,
    case::root_dir(vec!["pwd"], None, None, Ok("{ROOT}/test_data\n".as_bytes())),
)]
#[cfg_attr(
    unix,
    case::cwd(
        vec!["pwd"],
        Some(".."), // We start in the test_data dir
        None,
        Ok("{ROOT}\n".as_bytes()),
    ),
)]
#[case::stdin(
    vec!["cat", "-"], None, Some("test".as_bytes()), Ok("test".as_bytes()),
)]
#[case::binary_output(
    vec!["cat", "-"],
    None,
    Some(invalid_utf8()),
    Ok(invalid_utf8()),
)]
#[case::error_empty(
    vec![],
    None,
    None,
    Err("Command must have at least one element"),
)]
#[case::error_bad_command(
    vec!["fake"],
    None,
    None,
    Err("Executing command `fake`"),
)]
#[case::error_exit_code(
    vec!["ls", "--fake"],
    None,
    None,
    Err("Command `ls --fake` exited with"),
)]
#[tokio::test]
async fn test_command(
    #[case] command: Vec<&str>,
    #[case] cwd: Option<&str>,
    #[case] stdin: Option<&'static [u8]>,
    #[case] expected: Result<&[u8], &str>,
) {
    let template = Template::function_call(
        "command",
        [command.into_iter().map(Expression::from).collect()],
        [
            ("cwd", cwd.map(Expression::from)),
            ("stdin", stdin.map(Expression::from)),
        ],
    );
    // Replace {ROOT} with the root dir
    let expected = expected.map(|bytes| {
        if let Ok(s) = std::str::from_utf8(bytes) {
            s.replace("{ROOT}", &get_repo_root().to_string_lossy())
                .into_bytes()
        } else {
            bytes.to_owned()
        }
    });
    assert_result(
        template
            .render_bytes(&TemplateContext::factory(()).streaming(false))
            .await,
        expected,
    );
}

/// Test that the command isn't spawned until the output stream is evaluated.
/// This ensures we don't execute the command for previews
#[tokio::test]
async fn test_command_lazy() {
    let template = Template::function_call(
        "command",
        [vec!["i-will-fail".into()].into()],
        [],
    );
    // This shouldn't fail because the command isn't evaluated yet
    let output = template
        .render(&TemplateContext::factory(()).streaming(true))
        .await;
    assert_eq!(
        output.stream_source(),
        Some(&StreamSource::Command {
            command: vec!["i-will-fail".into()]
        })
    );
    let stream = output.try_into_stream().unwrap();

    // Error happens when we collect
    assert_result(
        stream.try_collect::<BytesMut>().await,
        Err::<BytesMut, &str>(if cfg!(unix) {
            "No such file or directory"
        } else {
            "program not found"
        }),
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
        template
            .render_string(&TemplateContext::factory(()).streaming(false))
            .await,
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
        template
            .render_bytes(&TemplateContext::factory(()).streaming(false))
            .await,
        expected,
    );
}

/// `file()`
#[rstest]
#[case::text("data.txt", Ok("text".as_bytes()))]
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

    // Path should be relative to the context's root dir
    let template = Template::function_call("file", [path.into()], []);
    let context = TemplateContext {
        root_dir: temp_dir.to_owned(),
        ..TemplateContext::factory(())
    };

    assert_result(
        template.render_bytes(&context.streaming(false)).await,
        expected,
    );
}

/// Bonus test case for ~ expansion in file(). Only test on Linux because
/// setting the home dir on Windows is annoying. As long as we call expand_home
/// we can trust it will work
#[cfg(unix)]
#[rstest]
#[tokio::test]
async fn test_file_tilde(temp_dir: TempDir) {
    fs::write(temp_dir.join("data.txt"), "text").await.unwrap();

    // Path should be relative to the context's root dir
    let template = Template::function_call("file", ["~/data.txt".into()], []);
    let context = TemplateContext {
        root_dir: temp_dir.to_owned(),
        ..TemplateContext::factory(())
    };

    let guard =
        env_lock::lock_env([("HOME", Some(temp_dir.to_str().unwrap()))]);
    assert_result(
        template.render_string(&context.streaming(false)).await,
        Ok("text"),
    );
    drop(guard);
}

/// `float()`
#[rstest]
#[case::null(Expression::Literal(Literal::Null), Ok(0.0))]
#[case::float(42.5.into(), Ok(42.5))]
#[case::int(42.into(), Ok(42.0))]
#[case::string("42.5".into(), Ok(42.5))]
#[case::string_int("42".into(), Ok(42.0))]
#[case::string_invalid("  42.5  ".into(), Err("invalid float literal"))]
#[case::bytes(b"42.5".into(), Ok(42.5))]
#[case::bytes_invalid(invalid_utf8().into(), Err("invalid utf-8 sequence"))]
#[case::bool_false(false.into(), Ok(0.0))]
#[case::bool_true(true.into(), Ok(1.0))]
#[case::array(
    vec!["1".into(), "2".into()].into(),
    Err("Expected one of float, integer, boolean, or string/bytes"),
)]
#[tokio::test]
async fn test_float(
    #[case] input: Expression,
    #[case] expected: Result<f64, &str>,
) {
    let template = Template::function_call("float", [input], []);
    assert_result(
        template
            .render(&TemplateContext::factory(()).streaming(false))
            .await
            .try_collect_value()
            .await,
        expected.map(Value::from),
    );
}

/// `integer()`
#[rstest]
#[case::null(Expression::Literal(Literal::Null), Ok(0))]
#[case::float(42.5.into(), Ok(42))]
#[case::int(42.into(), Ok(42))]
#[case::string("42".into(), Ok(42))]
#[case::string_float("42.5".into(), Err("invalid digit"))]
#[case::string_invalid("  42  ".into(), Err("invalid digit"))]
#[case::bytes(b"42".into(), Ok(42))]
#[case::bytes_invalid(invalid_utf8().into(), Err("invalid utf-8 sequence"))]
#[case::bool_false(false.into(), Ok(0))]
#[case::bool_true(true.into(), Ok(1))]
#[case::array(
    vec!["1".into(), "2".into()].into(),
    Err("Expected one of integer, float, boolean, or string/bytes"),
)]
#[tokio::test]
async fn test_integer(
    #[case] input: Expression,
    #[case] expected: Result<i64, &str>,
) {
    let template = Template::function_call("integer", [input], []);
    assert_result(
        template
            .render(&TemplateContext::factory(()).streaming(false))
            .await
            .try_collect_value()
            .await,
        expected.map(Value::from),
    );
}

/// `jq()`
#[rstest]
// Default mode is auto
#[case::mode_default_one(".[1]", None, None, Ok("b".into()))]
#[case::mode_default_many(".", None, None, Ok(vec!["a", "b", "c"].into()))]
#[case::error_auto_empty(
    "empty",
    Some("auto"),
    None,
    Err("No results from JSON query `empty`")
)]
#[case::mode_auto_one(".[1]", Some("auto"), None, Ok("b".into()))]
#[case::mode_auto_many(".[]", Some("auto"), None, Ok(vec!["a", "b", "c"].into()))]
#[case::mode_array_zero("empty", Some("array"), None, Ok(Value::Array(vec![])))]
#[case::mode_array_one(".[1]", Some("array"), None, Ok(vec!["b"].into()))]
#[case::mode_array_many(".[]", Some("array"), None, Ok(vec!["a", "b", "c"].into()))]
#[case::error_single_empty(
    "empty",
    Some("single"),
    None,
    Err("No results from JSON query `empty`")
)]
#[case::mode_single_one(".[1]", Some("single"), None, Ok("b".into()))]
#[case::error_single_many(
    ".[]",
    Some("single"),
    None,
    Err("Expected exactly one result from JSON query `.[]`, but got 3")
)]
#[case::error_parse(
    "does not parse",
    None,
    None,
    Err("expected nothing, got `not`")
)]
#[case::error_compile("asdf(.)", None, None, Err("Undefined function `asdf`"))]
#[case::error_runtime("error(\"bad!\")", None, None, Err("jq(): \"bad!\""))]
// Binary content can't be converted to JSON
#[case::error_binary(
    ".[1]",
    None,
    Some(invalid_utf8().into()),
    Err("Error parsing JSON")
)]
#[tokio::test]
async fn test_jq(
    #[case] query: &str,
    #[case] mode: Option<&str>,
    #[case] json: Option<Expression>, // If not given, use a default
    #[case] expected: Result<Value, &str>,
) {
    let json: Expression = json.unwrap_or_else(|| {
        vec!["a", "b", "c"]
            .into_iter()
            .map(Expression::from)
            .collect()
    });
    let template = Template::function_call(
        "jq",
        [query.into(), json],
        [("mode", mode.map(Expression::from))],
    );
    assert_result(
        template
            .render(&TemplateContext::factory(()).streaming(false))
            .await
            .try_collect_value()
            .await,
        expected,
    );
}

/// `json_parse()`
#[rstest]
#[case::object(br#"{"a": 1, "b": 2}"#, Ok(json!({"a": 1, "b": 2}).into()))]
#[case::string(br#""json string""#, Ok(json!("json string").into()))]
#[case::error_invalid_json(br#""unclosed"#, Err("EOF while parsing a string"))]
// Binary content can't be parsed
#[case::error_binary(invalid_utf8(), Err("invalid utf-8 sequence"))]
#[tokio::test]
async fn test_json_parse(
    #[case] json: &'static [u8],
    #[case] expected: Result<Value, &str>,
) {
    let template = Template::function_call("json_parse", [json.into()], []);
    assert_result(
        template
            .render(&TemplateContext::factory(()).streaming(false))
            .await
            .try_collect_value()
            .await,
        expected,
    );
}

/// `jsonpath()`
#[rstest]
// Default mode is auto
#[case::mode_default_one("$[1]", None, None, Ok("b".into()))]
#[case::mode_default_many("$[*]", None, None, Ok(vec!["a", "b", "c"].into()))]
#[case::error_auto_empty(
    "$[5]",
    Some("auto"),
    None,
    Err("No results from JSON query `$[5]`")
)]
#[case::mode_auto_one("$[1]", Some("auto"), None, Ok("b".into()))]
#[case::mode_auto_many("$[*]", Some("auto"), None, Ok(vec!["a", "b", "c"].into()))]
#[case::mode_array_zero("$[5]", Some("array"), None, Ok(Value::Array(vec![])))]
#[case::mode_array_one("$[1]", Some("array"), None, Ok(vec!["b"].into()))]
#[case::mode_array_many("$[*]", Some("array"), None, Ok(vec!["a", "b", "c"].into()))]
#[case::error_single_empty(
    "$[5]",
    Some("single"),
    None,
    Err("No results from JSON query `$[5]`")
)]
#[case::mode_single_one("$[1]", Some("single"), None, Ok("b".into()))]
#[case::error_single_many(
    "$[*]",
    Some("single"),
    None,
    Err("Expected exactly one result from JSON query `$[*]`, but got 3")
)]
#[case::error_invalid_query("bad query", None, None, Err("parser error"))]
// Binary content can't be converted to JSON
#[case::error_binary(
    "$[1]",
    None,
    Some(invalid_utf8().into()),
    Err("Error parsing JSON")
)]
#[tokio::test]
async fn test_jsonpath(
    #[case] query: &str,
    #[case] mode: Option<&str>,
    #[case] json: Option<Expression>, // If not given, use a default
    #[case] expected: Result<Value, &str>,
) {
    let json: Expression = json.unwrap_or_else(|| {
        vec!["a", "b", "c"]
            .into_iter()
            .map(Expression::from)
            .collect()
    });
    let template = Template::function_call(
        "jsonpath",
        [query.into(), json],
        [("mode", mode.map(Expression::from))],
    );
    assert_result(
        template
            .render(&TemplateContext::factory(()).streaming(false))
            .await
            .try_collect_value()
            .await,
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
    assert_result(
        template.render_bytes(&context.streaming(false)).await,
        expected,
    );
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

    assert_result(
        template.render_bytes(&context.streaming(false)).await,
        expected,
    );
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

    assert_result(
        template.render_bytes(&context.streaming(false)).await,
        expected,
    );
}

/// `select()`
#[rstest]
#[case::reply(
    vec!["first".into(), "second".into()],
    Some(1),
    Ok("second".into()),
)]
#[case::labelled(
    // Labelled objects are {"label": "Label", "value": "Value"}
    vec![
        [("label", "First".into()), ("value", 1.into())].into(),
        [("label", "Second".into()), ("value", 2.into())].into(),
    ],
    Some(1),
    Ok(2.into()), // value is returned, not label
)]
#[case::empty(vec![], None, Err("Select has no options"))]
#[case::no_reply(vec!["test".into()], None, Err("No reply"))]
#[tokio::test]
async fn test_select(
    #[case] options: Vec<Expression>,
    #[case] select: Option<usize>,
    #[case] expected: Result<Value, &str>,
) {
    let template = Template::function_call("select", [options.into()], []);
    let context = TemplateContext {
        prompter: Box::new(TestSelectPrompter::new(select.into_iter())),
        ..TemplateContext::factory(())
    };
    assert_result(
        template
            .render(&context.streaming(false))
            .await
            .try_collect_value()
            .await,
        expected,
    );
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
    assert_eq!(
        template
            .render_bytes(&context.streaming(false))
            .await
            .unwrap(),
        expected
    );
}

/// `string()`
#[rstest]
#[case::primitive(true.into(), Ok("true"))]
#[case::string("test".into(), Ok("test"))]
#[case::bytes(b"test".into(), Ok("test"))]
#[case::array(vec!["a".into(), "b".into()].into(), Ok("['a', 'b']"))]
#[case::error_invalid_utf8(invalid_utf8().into(), Err("invalid utf-8"))]
#[tokio::test]
async fn test_string(
    #[case] input: Expression,
    #[case] expected: Result<&str, &str>,
) {
    let template = Template::function_call("string", [input], []);
    assert_result(
        template
            .render_string(&TemplateContext::factory(()).streaming(false))
            .await,
        expected,
    );
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
            .render_bytes(&TemplateContext::factory(()).streaming(false))
            .await
            .unwrap(),
        expected
    );
}

/// Test that the stream source is retained for a single-chunk template
#[rstest]
#[case::stream_root("{{ file('data.json') }}", true)]
#[case::stream_piped("{{ 'data.json' | file() }}", true)]
#[case::stream_via_profile("{{ file_field }}", true)]
// Multiple chunks means we don't have a single stream source
#[case::no_stream_not_root("data: {{ file('data.json') }}", false)]
#[case::no_stream_not_root_via_profile("data: {{ file_field }}", false)]
#[tokio::test]
async fn test_stream_source(
    #[case] template: Template,
    #[case] expected_has_source: bool,
) {
    // Put some profile data in the context
    let profile_data = indexmap! {
        "file_field".into() => "{{ file('data.json') }}".into(),
    };
    let profile = Profile {
        data: profile_data,
        ..Profile::factory(())
    };
    let context = TemplateContext::factory((by_id([profile]), IndexMap::new()));

    let output = template.render(&context.streaming(true)).await;
    if expected_has_source {
        assert_matches!(output.stream_source(), Some(_));
    } else {
        assert_matches!(output.stream_source(), None);
    }
}

/// Test that streamed templates are actually computed chunk-by-chunk and are
/// never eagerly collected
#[rstest]
#[tokio::test]
async fn test_stream_chunks(temp_dir: TempDir) {
    // Put some profile data in the context
    let profile_data = indexmap! {
        "one_chunk".into() => "{{ file('second') }}".into(),
        // This one has multiple chunks. Need to make sure this doesn't get
        // collected, and the whole stream is passed through
        "multi_chunk".into() => "{{ file('third') }} | {{ file('fourth') }}".into(),
    };
    let profile = Profile {
        data: profile_data,
        ..Profile::factory(())
    };
    let context = TemplateContext {
        root_dir: temp_dir.to_owned(),
        ..TemplateContext::factory((by_id([profile]), IndexMap::new()))
    };

    // Testing that streaming directly or via a profile field loads the value
    // lazily
    let template = Template::from(
        "{{ file('first') }} | {{ one_chunk }} | {{ multi_chunk }}",
    );

    // Stream init succeeds even though the files don't exist yet, because they
    // aren't loaded until the respective chunk is loaded
    let mut stream = template
        .render(&context.streaming(true))
        .await
        .try_into_stream()
        .unwrap();
    // Convert chunks to strings for better assertions
    let mut next_chunk = async move || {
        stream.next().await.map(|result| {
            let bytes = result.unwrap();
            String::from_utf8(bytes.into()).unwrap()
        })
    };

    let write_file = async |name: &str| {
        fs::write(temp_dir.join(name), name).await.unwrap();
    };
    // Stream should be 3 chunks. Each chunk isn't computed until requested.
    // By creating the files right before reading, we assure that it isn't
    // loaded until the chunk is requested
    write_file("first").await;
    assert_eq!(next_chunk().await.as_deref(), Some("first"));

    assert_eq!(next_chunk().await.as_deref(), Some(" | "));

    // Profile field with one chunk
    write_file("second").await;
    assert_eq!(next_chunk().await.as_deref(), Some("second"));

    assert_eq!(next_chunk().await.as_deref(), Some(" | "));

    // Profile field with multiple chunks
    write_file("third").await;
    assert_eq!(next_chunk().await.as_deref(), Some("third"));
    assert_eq!(next_chunk().await.as_deref(), Some(" | "));
    write_file("fourth").await;
    assert_eq!(next_chunk().await.as_deref(), Some("fourth"));

    // Job's done
    assert_eq!(next_chunk().await, None);
}

/// Bytes that can't be converted to a string
fn invalid_utf8() -> &'static [u8] {
    b"\xc3\x28"
}
