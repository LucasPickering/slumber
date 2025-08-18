//! Tests for the http module

use super::*;
use crate::{
    collection::{Authentication, Profile},
    test_util::{TestPrompter, by_id, header_map, http_engine, invalid_utf8},
};
use indexmap::{IndexMap, indexmap};
use pretty_assertions::assert_eq;
use regex::Regex;
use reqwest::{Body, StatusCode, header};
use rstest::rstest;
use serde_json::json;
use slumber_util::{Factory, assert_err};
use std::ptr;
use wiremock::{Mock, MockServer, ResponseTemplate, matchers};

/// Create a template context. Take a set of extra recipes to add to the created
/// collection
fn template_context(recipe: Recipe) -> TemplateContext {
    let profile_data = indexmap! {
        "host".into() => "http://localhost".into(),
        "mode".into() => "sudo".into(),
        "user_id".into() => "1".into(),
        "group_id".into() => "3".into(),
        "username".into() => "user".into(),
        "password".into() => "hunter2".into(),
        "token".into() => "tokenzzz".into(),
        "prompt".into() => "{{ prompt() }}".into(),
        "error".into() => "{{ fake_fn() }}".into(),
    };
    let profile = Profile {
        data: profile_data,
        ..Profile::factory(())
    };
    TemplateContext {
        prompter: Box::new(TestPrompter::new(["first", "second"])),
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

/// Create a mock HTTP server and return its URL
async fn mock_server() -> String {
    // Mock HTTP response
    let server = MockServer::start().await;
    let host = server.uri();
    Mock::given(matchers::method("GET"))
        .and(matchers::path("/get"))
        .respond_with(
            ResponseTemplate::new(StatusCode::OK).set_body_string("hello!"),
        )
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
    host
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
    let context = template_context(recipe);

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
    let context = template_context(recipe);
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
    let context = template_context(Recipe {
        body: Some(body),
        ..Recipe::factory(())
    });
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
    let context = template_context(recipe);

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

/// Test each possible type of body. Raw bodies are covered by
/// [test_build_request]. This seems redundant with [test_build_body], but
/// we need this to test that the `content-type` header is set correctly.
/// This also allows us to test the actual built request, which could
/// hypothetically vary from the request record.
#[rstest]
#[case::json(
    RecipeBody::json(json!({"group_id": "{{ group_id }}"})).unwrap(),
    None,
    Some(b"{\"group_id\":\"3\"}".as_slice()),
    "^application/json$",
    &[],
)]
// Content-Type has been overridden by an explicit header
#[case::json_content_type_override(
    RecipeBody::json(json!({"group_id": "{{ group_id }}"})).unwrap(),
    Some("text/plain"),
    Some(br#"{"group_id":"3"}"#.as_slice()),
    "^text/plain$",
    &[],
)]
#[case::form_urlencoded(
    RecipeBody::FormUrlencoded(indexmap! {
        "user_id".into() => "{{ user_id }}".into(),
        "token".into() => "{{ token }}".into()
    }),
    None,
    Some(b"user_id=1&token=tokenzzz".as_slice()),
    "^application/x-www-form-urlencoded$",
    &[],
)]
// reqwest sets the content type when initializing the body, so make sure
// that doesn't override the user's value
#[case::form_urlencoded_content_type_override(
    RecipeBody::FormUrlencoded(Default::default()),
    Some("text/plain"),
    Some(b"".as_slice()),
    "^text/plain$",
    &[],
)]
#[case::form_multipart(
    RecipeBody::FormMultipart(indexmap! {
        "user_id".into() => "{{ user_id }}".into(),
        "binary".into() => invalid_utf8(),
    }),
    None,
    // multipart bodies are automatically turned into streams by reqwest,
    // and we don't store stream bodies atm
    // https://github.com/LucasPickering/slumber/issues/256
    None,
    "^multipart/form-data; boundary=[a-f0-9-]{67}$",
    &[("content-length", "321")],
)]
#[tokio::test]
async fn test_structured_body(
    http_engine: HttpEngine,
    #[case] body: RecipeBody,
    #[case] content_type: Option<&str>,
    #[case] expected_body: Option<&'static [u8]>,
    // For multipart bodies, the content type includes random content
    #[case] expected_content_type: Regex,
    #[case] extra_headers: &[(&str, &str)],
) {
    let headers = if let Some(content_type) = content_type {
        indexmap! {"content-type".into() => content_type.into()}
    } else {
        IndexMap::default()
    };
    let recipe = Recipe {
        headers,
        body: Some(body),
        ..Recipe::factory(())
    };
    let recipe_id = recipe.id.clone();
    let context = template_context(recipe);

    let seed = seed(&context, BuildOptions::default());
    let ticket = http_engine.build(seed, &context).await.unwrap();

    // Assert on the actual built request *and* the record, to make sure
    // they're consistent with each other
    let actual_content_type = ticket
        .request
        .headers()
        .get(header::CONTENT_TYPE)
        .expect("Missing Content-Type header")
        .to_str()
        .expect("Invalid Content-Type header");
    assert!(
        expected_content_type.is_match(actual_content_type),
        "Expected Content-Type `{actual_content_type}` \
            to match `{expected_content_type}`"
    );
    assert_eq!(
        ticket.request.body().and_then(Body::as_bytes),
        expected_body
    );

    assert_eq!(
        *ticket.record,
        RequestRecord {
            id: ticket.record.id,
            body: expected_body.map(Bytes::from),
            // Use the actual content type here, because the expected
            // content type maybe be a pattern and we need an exactl string.
            // We checked actual=expected above so this is fine
            headers: header_map(
                [("content-type", actual_content_type)]
                    .into_iter()
                    .chain(extra_headers.iter().copied())
            ),
            ..RequestRecord::factory((
                Some(context.collection.first_profile_id().clone()),
                recipe_id
            ))
        }
    );
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
    let context = template_context(recipe);

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
    let context = template_context(recipe);
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
    let context = template_context(recipe);
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
    let context = template_context(recipe);
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
    let context = template_context(recipe);

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
    let context = template_context(recipe);

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
#[tokio::test]
async fn test_profile_duplicate(http_engine: HttpEngine) {
    let recipe = Recipe {
        method: HttpMethod::Post,
        url: "{{ host }}/{{ prompt }}/{{ prompt }}".into(),
        body: Some("{{ prompt }}".into()),
        ..Recipe::factory(())
    };
    let context = template_context(recipe);

    let seed = seed(&context, BuildOptions::default());
    let ticket = http_engine.build(seed, &context).await.unwrap();

    let expected_url: Url = "http://localhost/first/first".parse().unwrap();
    let expected_body = "first";

    let request = &ticket.request;
    assert_eq!(request.url(), &expected_url);
    assert_eq!(
        request
            .body()
            .and_then(|body| std::str::from_utf8(body.as_bytes()?).ok()),
        Some(expected_body)
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
    let context = template_context(recipe);

    let seed = RequestSeed::new(recipe_id, BuildOptions::default());
    assert_err!(
        http_engine
            .build(seed, &context)
            .await
            // Include full error chain in the message
            .map_err(anyhow::Error::from),
        "fake_fn(): Unknown function"
    );
}

/// Test launching a built request
#[rstest]
#[tokio::test]
async fn test_send_request(http_engine: HttpEngine) {
    let host = mock_server().await;
    let recipe = Recipe {
        url: format!("{host}/get").as_str().into(),
        ..Recipe::factory(())
    };
    let context = template_context(recipe);
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
    let context = template_context(Recipe::factory(()));
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
    let context = template_context(recipe);
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
    let context = template_context(recipe);
    let seed = seed(&context, BuildOptions::default());
    let command = http_engine.build_curl(seed, &context).await.unwrap();
    let expected_command = format!(
        "curl -XGET --url 'http://localhost/url' {expected_arguments}",
    );
    assert_eq!(command, expected_command);
}

/// Build a curl command with each possible type of body
#[rstest]
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
    let context = template_context(recipe);

    let seed = seed(&context, BuildOptions::default());
    let command = http_engine.build_curl(seed, &context).await.unwrap();
    let expected_command =
        format!("curl -XGET --url 'http://localhost/url' {expected_arguments}");
    assert_eq!(command, expected_command);
}

/// By default, the engine will follow 3xx redirects
#[rstest]
#[tokio::test]
async fn test_follow_redirects(http_engine: HttpEngine) {
    let host = mock_server().await;
    let recipe = Recipe {
        url: format!("{host}/redirect").as_str().into(),
        ..Recipe::factory(())
    };
    let context = template_context(recipe);
    let seed = seed(&context, BuildOptions::default());

    // Build+send the request
    let ticket = http_engine.build(seed, &context).await.unwrap();
    let exchange = ticket.send().await.unwrap();

    // Should hit /redirect which redirects to /get, which returns the body
    assert_eq!(exchange.response.status, StatusCode::OK);
    assert_eq!(exchange.response.body.bytes().as_ref(), b"hello!");
}

/// Client should not follow redirects when the config field is disabled
#[tokio::test]
async fn test_follow_redirects_disabled() {
    let http_engine = HttpEngine::new(&HttpEngineConfig {
        follow_redirects: false,
        ..Default::default()
    });
    let host = mock_server().await;
    let recipe = Recipe {
        url: format!("{host}/redirect").as_str().into(),
        ..Recipe::factory(())
    };
    let context = template_context(recipe);
    let seed = seed(&context, BuildOptions::default());

    // Build+send the request
    let ticket = http_engine.build(seed, &context).await.unwrap();
    let exchange = ticket.send().await.unwrap();

    assert_eq!(exchange.response.status, StatusCode::MOVED_PERMANENTLY);
}
