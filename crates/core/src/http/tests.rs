//! Tests for the http module

use super::*;
use crate::{
    collection::{Authentication, Profile},
    test_util::{TestPrompter, by_id, header_map, http_engine, invalid_utf8},
};
use indexmap::{IndexMap, indexmap};
use pretty_assertions::assert_eq;
use reqwest::{Body, StatusCode, header};
use rstest::rstest;
use serde_json::json;
use slumber_util::{Factory, assert_err, test_data_dir};
use std::{cell::RefCell, path, ptr};
use wiremock::{Mock, MockServer, ResponseTemplate, matchers};

thread_local! {
    /// Out-of-band communication that the render code uses to share the
    /// boundary used for whatever multipart form was rendered most recently.
    /// This is really hacky but it's the best solution I can think of.
    ///
    /// Some alternatives:
    /// - Control the randomness to make the boundary predictable. Reqwest
    ///   doesn't provide any way to do this.
    /// - Use a regex in expectations instead of a static string. That blows up
    ///   the dependency tree and also gives much worse assertion messages.
    /// - Set the boundary to a static value. Right now that's not possible but
    ///   if https://github.com/seanmonstar/reqwest/pull/2814 ever gets merged,
    ///   we can use form.set_boundary()
    pub static MULTIPART_BOUNDARY: RefCell<String> = RefCell::default();
}

/// Create a template context. Take a set of extra recipes to add to the created
/// collection
fn template_context(recipe: Recipe, host: Option<&str>) -> TemplateContext {
    let profile_data = indexmap! {
        "host".into() => host.unwrap_or("http://localhost").into(),
        "mode".into() => "sudo".into(),
        "user_id".into() => "1".into(),
        "group_id".into() => "3".into(),
        "username".into() => "user".into(),
        "password".into() => "hunter2".into(),
        "token".into() => "tokenzzz".into(),
        "test_data_dir".into() => test_data_dir().to_str().unwrap().into(),
        "prompt".into() => "{{ prompt() }}".into(),
        "stream".into() => "{{ file('data.json') }}".into(),
        // Streamed value that we can use to test deduping
        "stream_prompt".into() => "{{ file(concat([prompt(), '.txt'])) }}".into(),
        "error".into() => "{{ fake_fn() }}".into(),
    };
    let profile = Profile {
        data: profile_data,
        ..Profile::factory(())
    };
    TemplateContext {
        prompter: Box::new(TestPrompter::new(["first", "second"])),
        root_dir: test_data_dir(),
        ..TemplateContext::factory((by_id([profile]), by_id([recipe])))
    }
}

/// Construct a [RequestSeed] for the first recipe in the context
fn seed(context: &TemplateContext, build_options: BuildOptions) -> RequestSeed {
    RequestSeed::new(
        context.collection.first_recipe_id().clone(),
        build_options,
    )
}

/// Make sure we only use the dangerous client when we really expect to.
/// There's isn't an easy way to mock TLS errors, so the easiest way to
/// test this is to just make sure [HttpEngine::get_client] returns the
/// expected client
#[rstest]
#[case::safe("safe", false)]
#[case::danger("danger", true)]
fn test_get_client(
    http_engine: HttpEngine,
    #[case] hostname: &str,
    #[case] expected_danger: bool,
) {
    let client =
        http_engine.get_client(&format!("http://{hostname}/").parse().unwrap());
    if expected_danger {
        assert!(ptr::eq(
            client,
            &raw const http_engine.danger_client.as_ref().unwrap().0
        ));
    } else {
        assert!(ptr::eq(client, &raw const http_engine.client));
    }
}

#[rstest]
#[tokio::test]
async fn test_build_request(http_engine: HttpEngine) {
    let recipe = Recipe {
        method: HttpMethod::Post,
        url: "{{ host }}/users/{{ user_id }}".into(),
        query: indexmap! {
            "mode".into() => "{{ mode }}".into(),
            "fast".into() => "true".into(),
        },
        headers: indexmap! {
            // Leading/trailing newlines should be stripped
            "Accept".into() => "application/json".into(),
            "Content-Type".into() => "application/json".into(),
        },
        body: Some("{\"group_id\":\"{{ group_id }}\"}".into()),
        ..Recipe::factory(())
    };
    let recipe_id = recipe.id.clone();
    let context = template_context(recipe, None);

    let seed = seed(&context, BuildOptions::default());
    let ticket = http_engine.build(seed, &context).await.unwrap();

    let expected_url: Url = "http://localhost/users/1?mode=sudo&fast=true"
        .parse()
        .unwrap();
    let expected_headers = header_map([
        ("content-type", "application/json"),
        ("accept", "application/json"),
    ]);
    let expected_body = b"{\"group_id\":\"3\"}";

    // Assert on the actual request
    let request = &ticket.request;
    assert_eq!(request.method(), reqwest::Method::POST);
    assert_eq!(request.url(), &expected_url);
    assert_eq!(request.headers(), &expected_headers);
    assert_eq!(
        request.body().and_then(Body::as_bytes),
        Some(expected_body.as_slice())
    );

    // Assert on the record too, to make sure it matches
    assert_eq!(
        *ticket.record,
        RequestRecord {
            id: ticket.record.id,
            profile_id: Some(context.collection.first_profile_id().clone()),
            recipe_id,
            method: HttpMethod::Post,
            http_version: HttpVersion::Http11,
            url: expected_url,
            body: Some(Vec::from(expected_body).into()),
            headers: expected_headers,
        }
    );
}

/// Test building just a URL. Should include query params, but headers/body
/// should *not* be built
#[rstest]
#[tokio::test]
async fn test_build_url(http_engine: HttpEngine) {
    let recipe = Recipe {
        url: "{{ host }}/users/{{ user_id }}".into(),
        query: indexmap! {
            "mode".into() => ["{{ mode }}", "user"].into(),
            "fast".into() => ["true", "false"].into(),
        },
        ..Recipe::factory(())
    };
    let context = template_context(recipe, None);
    let seed = seed(&context, BuildOptions::default());
    let url = http_engine.build_url(seed, &context).await.unwrap();

    assert_eq!(
        url.as_str(),
        "http://localhost/users/1?mode=sudo&mode=user&fast=true&fast=false"
    );
}

/// Test building just a body. URL/query/headers should *not* be built.
#[rstest]
#[case::raw(
    RecipeBody::Raw(r#"{"group_id":"{{ group_id }}"}"#.into()),
    br#"{"group_id":"3"}"#
)]
#[case::json(
    RecipeBody::json(json!({"group_id": "{{ group_id }}"})).unwrap(),
    br#"{"group_id":"3"}"#,
)]
#[case::binary(RecipeBody::Raw(invalid_utf8()), b"\xc3\x28")]
#[tokio::test]
async fn test_build_body(
    http_engine: HttpEngine,
    #[case] body: RecipeBody,
    #[case] expected_body: &[u8],
) {
    let context = template_context(
        Recipe {
            body: Some(body),
            ..Recipe::factory(())
        },
        None,
    );
    let seed = seed(&context, BuildOptions::default());
    let body = http_engine.build_body(seed, &context).await.unwrap();

    assert_eq!(body.as_deref(), Some(expected_body));
}

/// Test building requests with various authentication methods
#[rstest]
#[case::basic(
    Authentication::Basic {
        username: "{{ username }}".into(),
        password: Some("{{ password }}".into()),
    },
    "Basic dXNlcjpodW50ZXIy"
)]
#[case::basic_no_password(
    Authentication::Basic {
        username: "{{ username }}".into(),
        password: None,
    },
    "Basic dXNlcjo="
)]
#[case::bearer(Authentication::Bearer { token: "{{ token }}".into() }, "Bearer tokenzzz")]
#[tokio::test]
async fn test_authentication(
    http_engine: HttpEngine,
    #[case] authentication: Authentication,
    #[case] expected_header: &str,
) {
    let recipe = Recipe {
        // `Authorization` header should appear twice. This probably isn't
        // something a user would ever want to do, but it should be
        // well-defined
        headers: indexmap! {"Authorization".into() => "bogus".into()},
        authentication: Some(authentication),
        ..Recipe::factory(())
    };
    let recipe_id = recipe.id.clone();
    let context = template_context(recipe, None);

    let seed = seed(&context, BuildOptions::default());
    let ticket = http_engine.build(seed, &context).await.unwrap();

    assert_eq!(
        *ticket.record,
        RequestRecord {
            id: ticket.record.id,
            profile_id: Some(context.collection.first_profile_id().clone()),
            recipe_id,
            method: HttpMethod::Get,
            http_version: HttpVersion::Http11,
            url: "http://localhost/url".parse().unwrap(),
            headers: header_map([
                ("authorization", "bogus"),
                ("authorization", expected_header)
            ]),
            body: None,
        }
    );
}

/// Test each possible type of body. This seems redundant with
/// [test_build_body], but we need this to test that the `content-type` header
/// is set correctly. This also allows us to test the actual built request,
/// which could hypothetically vary from the request record.
#[rstest]
#[case::text(RecipeBody::Raw("hello!".into()), None, None, "hello!")]
#[case::json(
    RecipeBody::json(json!({"group_id": "{{ group_id }}"})).unwrap(),
    None,
    Some("application/json"),
    r#"{"group_id":"3"}"#,
)]
// Content-Type has been overridden by an explicit header
#[case::json_content_type_override(
    RecipeBody::json(json!({"group_id": "{{ group_id }}"})).unwrap(),
    Some("text/plain"),
    Some("text/plain"),
    r#"{"group_id":"3"}"#,
)]
#[case::json_unpack(
    // Single-chunk templates should get unpacked to the actual JSON value
    // instead of returned as a string
    RecipeBody::json(json!("{{ [1,2,3] }}")).unwrap(),
    None,
    Some("application/json"),
    "[1,2,3]",
)]
#[case::json_no_unpack(
    // This template doesn't get unpacked because it is multiple chunks
    RecipeBody::json(json!("no: {{ [1,2,3] }}")).unwrap(),
    None,
    Some("application/json"),
    // Spaces are added because this uses the template Value stringification
    // instead of serde_json stringification
    r#""no: [1, 2, 3]""#,
)]
#[case::json_string_from_file(
    // JSON data is loaded as a string and NOT unpacked. file() returns bytes
    // which automatically get interpreted as a string.
    RecipeBody::json(json!(
        "{{ file(concat([test_data_dir, '/data.json'])) | trim() }}"
    )).unwrap(),
    None,
    Some("application/json"),
    r#""{ \"a\": 1, \"b\": 2 }""#,
)]
#[case::json_from_file_parsed(
    // Pipe to json_parse() to parse it
    RecipeBody::json(json!(
        "{{ file(concat([test_data_dir, '/data.json'])) | json_parse() }}"
    )).unwrap(),
    None,
    Some("application/json"),
    r#"{"a":1,"b":2}"#,
)]
#[case::form_urlencoded(
    RecipeBody::FormUrlencoded(indexmap! {
        "user_id".into() => "{{ user_id }}".into(),
        "token".into() => "{{ token }}".into()
    }),
    None,
    Some("application/x-www-form-urlencoded"),
    "user_id=1&token=tokenzzz",
)]
// reqwest sets the content type when initializing the body, so make sure
// that doesn't override the user's value
#[case::form_urlencoded_content_type_override(
    RecipeBody::FormUrlencoded(Default::default()),
    Some("text/plain"),
    Some("text/plain"),
    ""
)]
#[tokio::test]
async fn test_body(
    http_engine: HttpEngine,
    #[case] body: RecipeBody,
    #[case] content_type: Option<&str>,
    // Expected value of the request's Content-Type header
    #[case] expected_content_type: Option<&str>,
    // Expected value of the request body
    #[case] expected_body: &'static str,
) {
    let headers = if let Some(content_type) = content_type {
        indexmap! {"content-type".into() => content_type.into()}
    } else {
        IndexMap::default()
    };
    let recipe = Recipe {
        method: HttpMethod::Post,
        url: "{{ host }}/post".into(),
        headers,
        body: Some(body),
        ..Recipe::factory(())
    };
    let context = template_context(recipe, None);

    let seed = seed(&context, BuildOptions::default());
    let ticket = http_engine.build(seed, &context).await.unwrap();
    let request = ticket.record;

    assert_eq!(
        request
            .headers
            .get("Content-Type")
            .map(|value| value.to_str().unwrap()),
        expected_content_type
    );
    // Convert body to text for comparison, because it gives better errors
    let body = request.body.as_ref().expect("Expected request body");
    let body_text = std::str::from_utf8(body).unwrap();
    assert_eq!(body_text, expected_body);
}

/// Test request bodies that are streamed. Streaming means the body is never
/// loaded entirely into memory at once.
#[rstest]
#[case::stream_static(
    RecipeBody::Stream("static string".into()),
    None,
    "static string",
)]
#[case::stream_file(
    RecipeBody::Stream("{{ file('data.json') }}".into()),
    None, // Content-Type is intentionally *not* inferred from the extension
    r#"{ "a": 1, "b": 2 }"#,
)]
#[case::stream_command(
    RecipeBody::Stream("{{ command(['cat', 'data.json']) }}".into()),
    None,
    r#"{ "a": 1, "b": 2 }"#,
)]
#[case::stream_profile(
    // Profile field should *not* eagerly resolve the stream
    RecipeBody::Stream("{{ stream }}".into()),
    None,
    r#"{ "a": 1, "b": 2 }"#,
)]
#[case::stream_multichunk(
    // This gets streamed one chunk at a time
    RecipeBody::Stream(r#"{ "data": {{ file('data.json') }} }"#.into()),
    None,
    r#"{ "data": { "a": 1, "b": 2 } }"#,
)]
#[case::form_multipart(
    RecipeBody::FormMultipart(indexmap! {
        "user_id".into() => "{{ user_id }}".into(),
    }),
    // Normally the boundary is random, but we make it static for testing
    Some("multipart/form-data; boundary={BOUNDARY}"),
    "--{BOUNDARY}\r
Content-Disposition: form-data; name=\"user_id\"\r
\r
1\r
--{BOUNDARY}--\r
",
)]
#[case::form_multipart_file(
    RecipeBody::FormMultipart(indexmap! {
        "file".into() => "{{ file('data.json') }}".into(),
    }),
    Some("multipart/form-data; boundary={BOUNDARY}"),
    "--{BOUNDARY}\r
Content-Disposition: form-data; name=\"file\"; filename=\"data.json\"\r
Content-Type: application/json\r
\r
{ \"a\": 1, \"b\": 2 }\r
--{BOUNDARY}--\r
",
)]
#[case::form_multipart_file_multichunk(
    RecipeBody::FormMultipart(indexmap! {
        // This body gets streamed, but it does *not* use native file support
        // because it's not *just* the file
        "file".into() => "data: {{ file('data.json') }}".into(),
    }),
    Some("multipart/form-data; boundary={BOUNDARY}"),
    "--{BOUNDARY}\r
Content-Disposition: form-data; name=\"file\"\r
\r
data: { \"a\": 1, \"b\": 2 }\r
--{BOUNDARY}--\r
",
)]
#[case::form_multipart_command(
    RecipeBody::FormMultipart(indexmap! {
        "command".into() => "{{ command(['cat', 'data.json']) }}".into(),
    }),
    Some("multipart/form-data; boundary={BOUNDARY}"),
    "--{BOUNDARY}\r
Content-Disposition: form-data; name=\"command\"\r
\r
{ \"a\": 1, \"b\": 2 }\r
--{BOUNDARY}--\r
",
)]
#[tokio::test]
async fn test_body_stream(
    http_engine: HttpEngine,
    #[case] body: RecipeBody,
    #[case] expected_content_type: Option<&str>,
    // Expected value of the request body
    #[case] expected_body: &'static str,
) {
    // Streamed bodies aren't stored on the request, so we're going to actually
    // send the request and echo the body back in the response
    let server = MockServer::start().await;
    Mock::given(matchers::method("POST"))
        .and(matchers::path("/post"))
        .respond_with(move |request: &wiremock::Request| {
            // Echo back the Content-Type and body so we can assert on it
            let mut response = ResponseTemplate::new(StatusCode::OK)
                .set_body_bytes(request.body.clone());
            if let Some(content_type) =
                request.headers.get(header::CONTENT_TYPE)
            {
                response =
                    response.append_header(header::CONTENT_TYPE, content_type);
            }
            response
        })
        .mount(&server)
        .await;

    let recipe = Recipe {
        method: HttpMethod::Post,
        url: "{{ host }}/post".into(),
        body: Some(body),
        ..Recipe::factory(())
    };
    let context = template_context(recipe, Some(&server.uri()));

    let seed = seed(&context, BuildOptions::default());
    let ticket = http_engine.build(seed, &context).await.unwrap();

    // The rendering code should set the correct boundary in TLS
    let (expected_content_type, expected_body) = MULTIPART_BOUNDARY
        .with_borrow(|boundary| {
            (
                expected_content_type
                    .map(|s| s.replace("{BOUNDARY}", boundary)),
                expected_body.replace("{BOUNDARY}", boundary),
            )
        });

    let exchange = ticket.send().await.unwrap();

    // Note: this doesn't actually enforce that the body was streamed
    // chunk-by-chunk, we just know that the right bytes got there in the end.
    // There's a test in the render module for that.

    // Mocker echoes the Content-Type header and body, assert on them
    assert_eq!(exchange.response.status, StatusCode::OK);
    let actual_content_type =
        exchange.response.headers.get(header::CONTENT_TYPE).map(
            |content_type| {
                content_type.to_str().expect("Invalid Content-Type header")
            },
        );
    assert_eq!(
        actual_content_type,
        expected_content_type.as_deref(),
        "Incorrect Content-Type header"
    );
    let body = exchange.response.body.text().expect("Invalid UTF-8 body");
    assert_eq!(body, expected_body, "Incorrect body");
}

/// Test overriding URL in BuildOptions
#[rstest]
#[tokio::test]
async fn test_override_url(http_engine: HttpEngine) {
    let recipe = Recipe::factory(());
    let context = template_context(recipe, None);

    let seed = seed(
        &context,
        BuildOptions {
            url: Some("http://custom-host/users/{{ username }}".into()),
            ..Default::default()
        },
    );
    let ticket = http_engine.build(seed, &context).await.unwrap();

    assert_eq!(ticket.record.url.as_str(), "http://custom-host/users/user");
}

/// Test overriding authentication in BuildOptions
#[rstest]
#[tokio::test]
async fn test_override_authentication(http_engine: HttpEngine) {
    let recipe = Recipe {
        authentication: Some(Authentication::Basic {
            username: "username".into(),
            password: None,
        }),
        ..Recipe::factory(())
    };
    let context = template_context(recipe, None);

    let seed = seed(
        &context,
        BuildOptions {
            authentication: Some(Authentication::Basic {
                username: "{{ username }}".into(),
                password: Some("{{ password }}".into()),
            }),
            ..Default::default()
        },
    );
    let ticket = http_engine.build(seed, &context).await.unwrap();

    assert_eq!(
        ticket
            .record
            .headers
            .get("Authorization")
            .and_then(|v| v.to_str().ok()),
        Some("Basic dXNlcjpodW50ZXIy")
    );
}

/// Test overriding headers in BuildOptions
#[rstest]
#[tokio::test]
async fn test_override_headers(http_engine: HttpEngine) {
    let recipe = Recipe {
        body: Some(RecipeBody::json(json!("test")).unwrap()),
        headers: indexmap! {
            // Included
            "Accept".into() => "application/json".into(),
            // Overridden
            "Big-Guy".into() => "style1".into(),
            // Excluded (replaced by default from body)
            "content-type".into() => "text/plain".into(),
        },
        ..Recipe::factory(())
    };
    let context = template_context(recipe, None);
    let seed = seed(
        &context,
        BuildOptions {
            headers: [
                (1, BuildFieldOverride::Override("style2".into())),
                (2, BuildFieldOverride::Omit),
            ]
            .into_iter()
            .collect(),
            ..Default::default()
        },
    );
    let ticket = http_engine.build(seed, &context).await.unwrap();

    assert_eq!(
        ticket.record.headers,
        header_map([
            ("accept", "application/json"),
            ("Big-Guy", "style2"),
            // It picked up the default content-type from the body,
            // because ours was excluded
            ("content-type", "application/json"),
        ])
    );
}

/// Test overriding query parameters in BuildOptions
#[rstest]
#[tokio::test]
async fn test_override_query(http_engine: HttpEngine) {
    let recipe = Recipe {
        url: "http://localhost/url".into(),
        query: indexmap! {
            // Overridden
            "mode".into() => "regular".into(),
            // Excluded
            "fast".into() => [
                "false", // Excluded
                "true", // Included
            ].into(),
        },
        ..Recipe::factory(())
    };
    let context = template_context(recipe, None);
    let seed = seed(
        &context,
        BuildOptions {
            query_parameters: [
                (0, BuildFieldOverride::Override("{{ mode }}".into())),
                (1, BuildFieldOverride::Omit),
            ]
            .into_iter()
            .collect(),
            ..Default::default()
        },
    );
    let ticket = http_engine.build(seed, &context).await.unwrap();

    // Should override "mode" and omit "fast=false"
    assert_eq!(
        ticket.record.url.as_str(),
        "http://localhost/url?mode=sudo&fast=true"
    );
}

/// Test overriding raw body in BuildOptions
#[rstest]
#[tokio::test]
async fn test_override_body_raw(http_engine: HttpEngine) {
    let recipe = Recipe {
        body: Some(RecipeBody::Raw("{{ username }}".into())),
        ..Recipe::factory(())
    };
    let context = template_context(recipe, None);
    let seed = seed(
        &context,
        BuildOptions {
            body: Some("{{ password }}".into()),
            ..Default::default()
        },
    );
    let ticket = http_engine.build(seed, &context).await.unwrap();

    assert_eq!(
        ticket
            .record
            .body
            .as_deref()
            .and_then(|bytes| std::str::from_utf8(bytes).ok()),
        Some("hunter2")
    );
}

/// Test overriding JSON body in BuildOptions
#[rstest]
#[tokio::test]
async fn test_override_body_json(http_engine: HttpEngine) {
    let recipe = Recipe {
        body: Some(
            RecipeBody::json(json!({"username": "{{ username }}"})).unwrap(),
        ),
        ..Recipe::factory(())
    };
    let context = template_context(recipe, None);

    let seed = seed(
        &context,
        BuildOptions {
            body: Some(RecipeBody::json(json!({"username": "user1"})).unwrap()),
            ..Default::default()
        },
    );
    let ticket = http_engine.build(seed, &context).await.unwrap();

    assert_eq!(
        ticket
            .record
            .body
            .as_deref()
            .and_then(|bytes| std::str::from_utf8(bytes).ok()),
        Some(r#"{"username":"user1"}"#)
    );
}

/// Test overriding form body fields. This has to be a separate test
/// because it's incompatible with testing raw body overrides
#[rstest]
#[tokio::test]
async fn test_override_body_form(http_engine: HttpEngine) {
    let recipe = Recipe {
        // This should implicitly set the content-type header
        body: Some(RecipeBody::FormUrlencoded(indexmap! {
            // Included
            "user_id".into() => "{{ user_id }}".into(),
            // Excluded
            "token".into() => "{{ token }}".into(),
            // Overridden
            "preference".into() => "large".into(),
        })),
        ..Recipe::factory(())
    };
    let recipe_id = recipe.id.clone();
    let context = template_context(recipe, None);

    let seed = seed(
        &context,
        BuildOptions {
            form_fields: [
                (1, BuildFieldOverride::Omit),
                (2, BuildFieldOverride::Override("small".into())),
            ]
            .into_iter()
            .collect(),
            ..Default::default()
        },
    );
    let ticket = http_engine.build(seed, &context).await.unwrap();

    assert_eq!(
        *ticket.record,
        RequestRecord {
            id: ticket.record.id,
            profile_id: context.selected_profile.clone(),
            recipe_id,
            method: HttpMethod::Get,
            http_version: HttpVersion::Http11,
            url: "http://localhost/url".parse().unwrap(),
            headers: header_map([(
                "content-type",
                "application/x-www-form-urlencoded"
            ),]),
            body: Some(b"user_id=1&preference=small".as_slice().into()),
        }
    );
}

/// Using the same profile field in two different templates should be
/// deduplicated, so that the expression is only evaluated once
#[rstest]
#[case::url_body(
    // Dedupe happens within a single template AND across templates
    "{{ host }}/{{ prompt }}/{{ prompt }}",
    "{{ prompt }}".into(),
    "first",
)]
#[case::url_multipart_body(
    "{{ host }}/{{ stream_prompt }}/{{ stream_prompt }}",
    // The body should *not* be streamed because is cached from the URL. This
    // works by rendering the body last
    RecipeBody::FormMultipart(indexmap!{
        "file".into() => "{{ stream_prompt }}".into(),
    }),
    "--{BOUNDARY}\r
Content-Disposition: form-data; name=\"file\"\r
\r
first\r
--{BOUNDARY}--\r
",
)]
#[case::multipart_body_multiple(
    "{{ host }}/first/first",
    // Field is used twice in the same body. The stream is *not* cloned, meaning
    // the prompt runs twice. This is a bug but requires a lot of machinery to
    // fix and in practice should be very rare. Why would you need to stream the
    // same source twice within the same form?
    RecipeBody::FormMultipart(indexmap!{
        "f1".into() => "{{ stream_prompt }}".into(),
        "f2".into() => "{{ stream_prompt }}".into(),
    }),
    "--{BOUNDARY}\r
Content-Disposition: form-data; name=\"f1\"; filename=\"first.txt\"\r
Content-Type: text/plain\r
\r
first\r
--{BOUNDARY}\r
Content-Disposition: form-data; name=\"f2\"; filename=\"second.txt\"\r
Content-Type: text/plain\r
\r
second\r
--{BOUNDARY}--\r
",
)]
#[tokio::test]
async fn test_profile_duplicate(
    http_engine: HttpEngine,
    #[case] url: Template,
    #[case] body: RecipeBody,
    #[case] expected_body: &str,
) {
    // We're going to actually send the request so we can get the full body.
    // Reqwest doesn't expose the body for multipart requests because it may be
    // streamed
    let server = MockServer::start().await;
    let host = server.uri();
    Mock::given(matchers::method("POST"))
        .and(matchers::path("/first/first"))
        .respond_with(|request: &wiremock::Request| {
            ResponseTemplate::new(StatusCode::OK)
                .set_body_bytes(request.body.clone())
        })
        .mount(&server)
        .await;

    let recipe = Recipe {
        method: HttpMethod::Post,
        url,
        body: Some(body),
        ..Recipe::factory(())
    };
    let context = template_context(recipe, Some(&host));

    let seed = seed(&context, BuildOptions::default());
    let ticket = http_engine.build(seed, &context).await.unwrap();

    // The rendering code should set the correct boundary in TLS
    let expected_body = MULTIPART_BOUNDARY
        .with_borrow(|boundary| expected_body.replace("{BOUNDARY}", boundary));

    // Make sure the URL rendered correctly before sending
    let expected_url: Url = format!("{host}/first/first").parse().unwrap();
    let exchange = ticket.send().await.unwrap();

    assert_eq!(exchange.response.status, StatusCode::OK);
    assert_eq!(exchange.request.url, expected_url);
    assert_eq!(
        // The response body is the same as the request body
        std::str::from_utf8(exchange.response.body.bytes()).ok(),
        Some(expected_body.as_str())
    );
}

/// If a profile field is rendered twice in two separate templates but the first
/// call fails, the second should fail as well
#[rstest]
#[tokio::test]
async fn test_profile_duplicate_error(http_engine: HttpEngine) {
    let recipe = Recipe {
        method: HttpMethod::Post,
        url: "{{ host }}/{{ error }}".into(),
        body: Some("{{ error }}".into()),
        ..Recipe::factory(())
    };
    let recipe_id = recipe.id.clone();
    let context = template_context(recipe, None);

    let seed = RequestSeed::new(recipe_id, BuildOptions::default());
    assert_err(
        http_engine.build(seed, &context).await,
        "fake_fn(): Unknown function",
    );
}

/// Test launching a built request
#[rstest]
#[tokio::test]
async fn test_send_request(http_engine: HttpEngine) {
    // Mock HTTP response
    let server = MockServer::start().await;
    Mock::given(matchers::method("GET"))
        .and(matchers::path("/get"))
        .respond_with(
            ResponseTemplate::new(StatusCode::OK).set_body_string("hello!"),
        )
        .mount(&server)
        .await;

    let recipe = Recipe {
        url: "{{ host }}/get".into(),
        ..Recipe::factory(())
    };
    let context = template_context(recipe, Some(&server.uri()));
    let seed = seed(&context, BuildOptions::default());

    // Build+send the request
    let ticket = http_engine.build(seed, &context).await.unwrap();
    let exchange = ticket.send().await.unwrap();

    // Cheat on this one, because we don't know exactly when the server
    // resolved it
    let date_header = exchange
        .response
        .headers
        .get("date")
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(
        *exchange.response,
        ResponseRecord {
            id: exchange.id,
            status: StatusCode::OK,
            headers: header_map([
                ("content-type", "text/plain"),
                ("content-length", "6"),
                ("date", date_header),
            ]),
            body: ResponseBody::new(b"hello!".as_slice().into())
        }
    );
}

/// Leading/trailing newlines should be stripped from rendered header
/// values. These characters are invalid and trigger an error, so we assume
/// they're unintentional and the user won't miss them.
#[rstest]
#[tokio::test]
async fn test_render_headers_strip() {
    let recipe = Recipe {
        // Leading/trailing newlines should be stripped
        headers: indexmap! {
            "Accept".into() => "application/json".into(),
            "Host".into() => "\n{{ host }}\n".into(),
        },
        ..Recipe::factory(())
    };
    let context = template_context(Recipe::factory(()), None);
    let rendered = recipe
        .render_headers(&BuildOptions::default(), &context)
        .await
        .unwrap();

    assert_eq!(
        rendered,
        header_map([
            ("Accept", "application/json"),
            // This is a non-sensical value, but it's good enough
            ("Host", "http://localhost"),
        ])
    );
}

#[rstest]
#[case::empty(&[], &[])]
#[case::start(&[0, 0, 1, 1], &[1, 1])]
#[case::end(&[1, 1, 0, 0], &[1, 1])]
#[case::both(&[0, 1, 0, 1, 0, 0], &[1, 0, 1])]
fn test_trim_bytes(#[case] bytes: &[u8], #[case] expected: &[u8]) {
    let mut bytes = bytes.to_owned();
    trim_bytes(&mut bytes, |b| b == 0);
    assert_eq!(&bytes, expected);
}

/// Build a curl command with query parameters and headers
#[rstest]
#[tokio::test]
async fn test_build_curl(http_engine: HttpEngine) {
    let recipe = Recipe {
        method: HttpMethod::Get,
        query: indexmap! {
            "mode".into() => "{{ mode }}".into(),
            "fast".into() => ["true", "false"].into(),
        },
        headers: indexmap! {
            "Accept".into() => "application/json".into(),
            "Content-Type".into() => "application/json".into(),
        },
        ..Recipe::factory(())
    };
    let context = template_context(recipe, None);
    let seed = seed(&context, BuildOptions::default());

    let command = http_engine.build_curl(seed, &context).await.unwrap();
    let expected_command = "curl -XGET \
    --url 'http://localhost/url?mode=sudo&fast=true&fast=false' \
    --header 'accept: application/json' \
    --header 'content-type: application/json'";
    assert_eq!(command, expected_command);
}

/// Build a curl command with each authentication type
#[rstest]
#[case::basic(
    Authentication::Basic {
        username: "{{ username }}".into(),
        password: Some("{{ password }}".into()),
    },
    "--user 'user:hunter2'",
)]
#[case::basic_no_password(
    Authentication::Basic {
        username: "{{ username }}".into(),
        password: None,
    },
    "--user 'user:'",
)]
#[case::bearer(
    Authentication::Bearer { token: "{{ token }}".into() },
    "--header 'authorization: Bearer tokenzzz'",
)]
#[tokio::test]
async fn test_build_curl_authentication(
    http_engine: HttpEngine,
    #[case] authentication: Authentication,
    #[case] expected_arguments: &str,
) {
    let recipe = Recipe {
        authentication: Some(authentication),
        ..Recipe::factory(())
    };
    let context = template_context(recipe, None);
    let seed = seed(&context, BuildOptions::default());
    let command = http_engine.build_curl(seed, &context).await.unwrap();
    let expected_command = format!(
        "curl -XGET --url 'http://localhost/url' {expected_arguments}",
    );
    assert_eq!(command, expected_command);
}

/// Build a curl command with each possible type of body
#[rstest]
#[case::text(RecipeBody::Raw("hello!".into()), "--data 'hello!'")]
#[case::stream(
    RecipeBody::Stream("{{ file('data.json') }}".into()),
    "--data '@{ROOT}/data.json'",
)]
#[case::json(
    RecipeBody::json(json!({"group_id": "{{ group_id }}"})).unwrap(),
    r#"--json '{"group_id":"3"}'"#
)]
#[case::form_urlencoded(
    RecipeBody::FormUrlencoded(indexmap! {
        "user_id".into() => "{{ user_id }}".into(),
        "token".into() => "{{ token }}".into()
    }),
    "--data-urlencode 'user_id=1' --data-urlencode 'token=tokenzzz'"
)]
#[case::form_multipart(
    // This doesn't support binary content because we can't pass it via cmd
    RecipeBody::FormMultipart(indexmap! {
        "user_id".into() => "{{ user_id }}".into(),
        "token".into() => "{{ token }}".into()
    }),
    "-F 'user_id=1' -F 'token=tokenzzz'"
)]
#[case::form_multipart_file(
    RecipeBody::FormMultipart(indexmap! {
        "file".into() => "{{ file('data.json') }}".into(),
    }),
    "-F 'file=@{ROOT}/data.json'"
)]
#[case::form_multipart_command(
    RecipeBody::FormMultipart(indexmap! {
        "command".into() => "{{ command(['cat', 'data.json']) }}".into(),
    }),
    r#"-F 'command={ "a": 1, "b": 2 }'"#
)]
#[tokio::test]
async fn test_build_curl_body(
    http_engine: HttpEngine,
    #[case] body: RecipeBody,
    #[case] expected_arguments: &str,
) {
    let recipe = Recipe {
        body: Some(body),
        ..Recipe::factory(())
    };
    let context = template_context(recipe, None);

    let seed = seed(&context, BuildOptions::default());
    let command = http_engine.build_curl(seed, &context).await.unwrap();
    let expected_arguments = expected_arguments
        // Dynamic replacements for system-specific contents
        .replace('/', path::MAIN_SEPARATOR_STR)
        .replace("{ROOT}", &context.root_dir.to_string_lossy());
    let expected_command =
        format!("curl -XGET --url 'http://localhost/url' {expected_arguments}");
    assert_eq!(command, expected_command);
}

/// Client should not follow redirects when the config field is disabled
#[rstest]
#[case::enabled(true, StatusCode::OK)]
#[case::disabled(false, StatusCode::MOVED_PERMANENTLY)]
#[tokio::test]
async fn test_follow_redirects(
    #[case] follow_redirects: bool,
    #[case] expected_status: StatusCode,
) {
    // Mock HTTP responses
    let server = MockServer::start().await;
    let host = server.uri();
    Mock::given(matchers::method("GET"))
        .and(matchers::path("/get"))
        .respond_with(ResponseTemplate::new(StatusCode::OK))
        .mount(&server)
        .await;
    Mock::given(matchers::method("GET"))
        .and(matchers::path("/redirect"))
        .respond_with(
            ResponseTemplate::new(StatusCode::MOVED_PERMANENTLY)
                .insert_header("Location", format!("{host}/get")),
        )
        .mount(&server)
        .await;

    let http_engine = HttpEngine::new(&HttpEngineConfig {
        follow_redirects,
        ..Default::default()
    });
    let recipe = Recipe {
        url: "{{ host }}/redirect".into(),
        ..Recipe::factory(())
    };
    let context = template_context(recipe, Some(&host));
    let seed = seed(&context, BuildOptions::default());

    // Build+send the request
    let ticket = http_engine.build(seed, &context).await.unwrap();
    let exchange = ticket.send().await.unwrap();

    assert_eq!(exchange.response.status, expected_status);
}
