//! Tests for the http module

use super::*;
use crate::{
    collection::{Authentication, Collection, Profile},
    test_util::{TestPrompter, by_id, header_map, http_engine, invalid_utf8},
};
use indexmap::{IndexMap, indexmap};
use pretty_assertions::assert_eq;
use regex::Regex;
use reqwest::{Body, StatusCode};
use rstest::rstest;
use serde_json::json;
use slumber_util::{Factory, assert_err};
use std::ptr;
use wiremock::{Mock, MockServer, ResponseTemplate, matchers};

/// Create a template context and seed for a request
fn template_context(
    recipe: Recipe,
    overrides: impl IntoIterator<Item = (OverrideKey, OverrideValue)>,
) -> (TemplateContext, RequestSeed) {
    let recipe_id = recipe.id.clone();
    let profile_data = indexmap! {
        "host".into() => "http://localhost".into(),
        "mode".into() => "sudo".into(),
        "user_id".into() => "1".into(),
        "group_id".into() => "3".into(),
        "username".into() => "user".into(),
        "password".into() => "hunter2".into(),
        "token".into() => "tokenzzz".into(),
    };
    let profile = Profile {
        data: profile_data,
        ..Profile::factory(())
    };
    let profile_id = profile.id.clone();
    let overrides = overrides.into_iter().collect::<IndexMap<_, _>>().into();
    let context = TemplateContext {
        collection: Collection {
            recipes: by_id([recipe]).into(),
            profiles: by_id([profile]),
        }
        .into(),
        selected_profile: Some(profile_id.clone()),
        prompter: Box::new(TestPrompter::new(["first", "second"])),
        overrides,
        ..TemplateContext::factory(())
    };
    let seed = RequestSeed::new(recipe_id);
    (context, seed)
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
    http_engine: &HttpEngine,
    #[case] hostname: &str,
    #[case] expected_danger: bool,
) {
    let client =
        http_engine.get_client(&format!("http://{hostname}/").parse().unwrap());
    if expected_danger {
        assert!(ptr::eq(
            client,
            &http_engine.danger_client.as_ref().unwrap().0
        ));
    } else {
        assert!(ptr::eq(client, &http_engine.client));
    }
}

#[rstest]
#[tokio::test]
async fn test_build_request(http_engine: &HttpEngine) {
    let recipe = Recipe {
        method: HttpMethod::Post,
        url: "{{host}}/users/{{user_id}}".into(),
        query: indexmap! {
            "mode".into() => "{{mode}}".into(),
            "fast".into() => "true".into(),
        },
        headers: indexmap! {
            // Leading/trailing newlines should be stripped
            "Accept".into() => "application/json".into(),
            "Content-Type".into() => "application/json".into(),
        },
        body: Some("{\"group_id\":\"{{group_id}}\"}".into()),
        ..Recipe::factory(())
    };
    let recipe_id = recipe.id.clone();
    let (context, seed) = template_context(recipe, []);
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
async fn test_build_url(http_engine: &HttpEngine) {
    let recipe = Recipe {
        url: "{{host}}/users/{{user_id}}".into(),
        query: indexmap! {
            "mode".into() => ["{{mode}}", "user"].into(),
            "fast".into() => ["true", "false"].into(),
        },
        ..Recipe::factory(())
    };
    let (context, seed) = template_context(recipe, []);

    let url = http_engine.build_url(seed, &context).await.unwrap();

    assert_eq!(
        url.as_str(),
        "http://localhost/users/1?mode=sudo&mode=user&fast=true&fast=false"
    );
}

/// Test building just a body. URL/query/headers should *not* be built.
#[rstest]
#[case::raw(
    RecipeBody::Raw {
        body: r#"{"group_id":"{{group_id}}"}"#.into(),
        content_type: None,
    },
    br#"{"group_id":"3"}"#
)]
#[case::json(
    RecipeBody::Raw {
        body: json!({"group_id": "{{group_id}}"}).into(),
        content_type: Some(ContentType::Json),
    },
    b"{\n  \"group_id\": \"3\"\n}",
)]
#[case::binary(
    RecipeBody::Raw {
        body: "{{chains.binary}}".into(),
        content_type: None,
    },
    b"\xc3\x28",
)]
#[tokio::test]
async fn test_build_body(
    http_engine: &HttpEngine,
    #[case] body: RecipeBody,
    #[case] expected_body: &[u8],
) {
    let (context, seed) = template_context(
        Recipe {
            body: Some(body),
            ..Recipe::factory(())
        },
        [],
    );
    let body = http_engine.build_body(seed, &context).await.unwrap();

    assert_eq!(body.as_deref(), Some(expected_body));
}

/// Test building requests with various authentication methods
#[rstest]
#[case::basic(
    Authentication::Basic {
        username: "{{username}}".into(),
        password: "{{password}}".into(),
    },
    "Basic dXNlcjpodW50ZXIy"
)]
#[case::bearer(Authentication::Bearer("{{token}}".into()), "Bearer tokenzzz")]
#[tokio::test]
async fn test_authentication(
    http_engine: &HttpEngine,
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
    let (context, seed) = template_context(recipe, []);
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
    RecipeBody::Raw {
        body: json!({"group_id": "{{group_id}}"}).into(),
        content_type: Some(ContentType::Json),
    },
    None,
    Some(b"{\n  \"group_id\": \"3\"\n}".as_slice()),
    "^application/json$",
    &[],
)]
// Content-Type has been overridden by an explicit header
#[case::json_content_type_override(
    RecipeBody::Raw {
        body: json!({"group_id": "{{group_id}}"}).into(),
        content_type: Some(ContentType::Json),
    },
    Some("text/plain"),
    Some(b"{\n  \"group_id\": \"3\"\n}".as_slice()),
    "^text/plain$",
    &[],
)]
#[case::form_urlencoded(
    RecipeBody::FormUrlencoded(indexmap! {
        "user_id".into() => "{{user_id}}".into(),
        "token".into() => "{{token}}".into()
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
        "user_id".into() => "{{user_id}}".into(),
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
    http_engine: &HttpEngine,
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
    let (context, seed) = template_context(recipe, []);
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

// TODO test override profile field

/// Test overriding URL
#[rstest]
#[tokio::test]
async fn test_override_url(http_engine: &HttpEngine) {
    let recipe = Recipe {
        // Additional query params should still be included. This is maybe
        // surprising, but we're overriding just the `url` field of the recipe
        query: indexmap! {
            "mode".into() => "regular".into(),
        },
        ..Recipe::factory(())
    };
    let (context, seed) = template_context(
        recipe,
        [(OverrideKey::Url, "http://localhost/new".into())],
    );
    let ticket = http_engine.build(seed, &context).await.unwrap();

    assert_eq!(
        ticket.record.url.as_str(),
        "http://localhost/new?mode=regular"
    );
}

/// Omitting URL should trigger an error. Can't send a request without a URL!
#[rstest]
#[tokio::test]
async fn test_override_omit_url(http_engine: &HttpEngine) {
    let (context, seed) = template_context(
        Recipe::factory(()),
        [(OverrideKey::Url, OverrideValue::Omit)],
    );
    assert_err!(
        http_engine.build(seed, &context).await,
        "URL cannot be omitted"
    );
}

/// Test disabling and overriding query params
#[rstest]
#[tokio::test]
async fn test_override_query_params(http_engine: &HttpEngine) {
    let recipe = Recipe {
        query: indexmap! {
            "mode".into() => "regular".into(), // Overridden
            "fast".into() => [
                "false", // Excluded
                "true", // Included
                "empty", // Overridden
            ].into(),
            // All overridden by one override
            "good".into() => ["false", "true"].into(),
        },
        ..Recipe::factory(())
    };
    let overrides = [
        (
            OverrideKey::Query("mode".into(), Some(0)),
            "turbo_time".into(),
        ),
        (
            OverrideKey::Query("fast".into(), Some(0)),
            OverrideValue::Omit,
        ),
        (
            OverrideKey::Query("fast".into(), Some(2)),
            "overridden".into(),
        ),
        // Should do nothing because it doesn't match a param in the recipe
        (
            OverrideKey::Query("fast".into(), Some(3)),
            "does nothing".into(),
        ),
        (OverrideKey::Query("good".into(), None), "maybe".into()),
    ];
    let (context, seed) = template_context(recipe, overrides);
    let ticket = http_engine.build(seed, &context).await.unwrap();

    assert_eq!(
        ticket.record.url.as_str(),
        "http://localhost/url\
            ?mode=turbo_time&fast=true&fast=overridden&good=maybe"
    );
}

/// Test disabling and overriding headers
#[rstest]
#[tokio::test]
async fn test_override_headers(http_engine: &HttpEngine) {
    let recipe = Recipe {
        headers: indexmap! {
            // Included
            "Accept".into() => "application/json".into(),
            // Overidden
            "Big-Guy".into() => "style1".into(),
            // Omitted
            "content-type".into() => "text/plain".into(),
        },
        body: Some(RecipeBody::Raw {
            body: "3".into(),
            content_type: Some(ContentType::Json),
        }),
        ..Recipe::factory(())
    };
    let overrides = [
        (OverrideKey::Header("Big-Guy".into()), "style2".into()),
        (
            OverrideKey::Header("content-type".into()),
            OverrideValue::Omit,
        ),
    ];
    let (context, seed) = template_context(recipe, overrides);
    let ticket = http_engine.build(seed, &context).await.unwrap();

    assert_eq!(
        ticket.record.headers,
        header_map([
            ("accept", "application/json"),
            ("Big-Guy", "style2"),
            // It picked up the default content-type from the body because ours
            // was omitted
            ("content-type", "application/json"),
        ])
    );
}

/// Test disabling and overriding basic authentication fields
#[rstest]
// Omitted username omits the entire header
#[case::omit_username(Some(OverrideValue::Omit), None, None)]
#[case::omit_password(
    None,
    Some(OverrideValue::Omit),
    Some("Basic dXNlcm5hbWU6"), // `username:`
)]
#[case::omit_both(Some(OverrideValue::Omit), Some(OverrideValue::Omit), None)]
#[case::override_username(
    Some("new username".into()),
    None,
    Some("Basic bmV3IHVzZXJuYW1lOnBhc3N3b3Jk"), // `new username:password`
)]
#[case::override_password(
    None,
    Some("new password".into()),
    Some("Basic dXNlcm5hbWU6bmV3IHBhc3N3b3Jk"), // `username:new password`
)]
#[tokio::test]
async fn test_override_authentication_basic(
    http_engine: &HttpEngine,
    #[case] username_override: Option<OverrideValue>,
    #[case] password_override: Option<OverrideValue>,
    #[case] expected_header: Option<&str>,
) {
    let recipe = Recipe {
        authentication: Some(Authentication::Basic {
            username: "username".into(),
            password: "password".into(),
        }),
        ..Recipe::factory(())
    };
    let overrides = [
        (OverrideKey::AuthenticationUsername, username_override),
        (OverrideKey::AuthenticationPassword, password_override),
    ]
    .into_iter()
    .filter_map(|(key, value)| Some((key, value?)));
    let (context, seed) = template_context(recipe, overrides);
    let ticket = http_engine.build(seed, &context).await.unwrap();

    assert_eq!(
        ticket
            .record
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|header| header.to_str().ok()),
        expected_header
    );
}

/// Test disabling and overriding bearer token
#[rstest]
#[case::omit(OverrideValue::Omit, None)]
#[case::override_token("new_token".into(), Some("Bearer new_token"))]
#[tokio::test]
async fn test_override_authentication_bearer(
    http_engine: &HttpEngine,
    #[case] token_override: OverrideValue,
    #[case] expected_header: Option<&str>,
) {
    let recipe = Recipe {
        authentication: Some(Authentication::Bearer("token".into())),
        ..Recipe::factory(())
    };
    let (context, seed) = template_context(
        recipe,
        [(OverrideKey::AuthenticationToken, token_override)],
    );
    let ticket = http_engine.build(seed, &context).await.unwrap();

    assert_eq!(
        ticket
            .record
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|header| header.to_str().ok()),
        expected_header
    );
}

/// Test disabling and overriding raw text bodies
#[rstest]
#[case::omit(OverrideValue::Omit, None)]
#[case::override_body("new data".into(), Some("new data"))]
#[tokio::test]
async fn test_override_body_raw(
    http_engine: &HttpEngine,
    #[case] body_override: OverrideValue,
    #[case] expected_body: Option<&str>,
) {
    let recipe = Recipe {
        body: Some(RecipeBody::Raw {
            body: "data".into(),
            content_type: None,
        }),
        ..Recipe::factory(())
    };
    let (context, seed) =
        template_context(recipe, [(OverrideKey::Body, body_override)]);
    let ticket = http_engine.build(seed, &context).await.unwrap();

    assert_eq!(ticket.record.body_str().ok().flatten(), expected_body);
}

/// Test disabling and overriding JSON bodies
#[rstest]
#[case::omit(OverrideValue::Omit, None)]
#[case::override_body("\"password\"".into(), Some("\"password\""))]
#[tokio::test]
async fn test_override_body_json(
    http_engine: &HttpEngine,
    #[case] body_override: OverrideValue,
    #[case] expected_body: Option<&str>,
) {
    let recipe = Recipe {
        body: Some(RecipeBody::Raw {
            body: "3".into(),
            content_type: Some(ContentType::Json),
        }),
        ..Recipe::factory(())
    };
    let (context, seed) =
        template_context(recipe, [(OverrideKey::Body, body_override)]);
    let ticket = http_engine.build(seed, &context).await.unwrap();

    assert_eq!(ticket.record.body_str().ok().flatten(), expected_body);
}

/// Test that the request fails to build if we override a JSON body with a
/// string that isn't valid JSON
#[rstest]
#[tokio::test]
async fn test_override_body_json_invalid(http_engine: &HttpEngine) {
    let recipe = Recipe {
        body: Some(RecipeBody::Raw {
            body: "3".into(),
            content_type: Some(ContentType::Json),
        }),
        ..Recipe::factory(())
    };
    let (context, seed) =
        template_context(recipe, [(OverrideKey::Body, "{invalid json".into())]);

    assert_err!(
        http_engine.build(seed, &context).await,
        "Error parsing body as JSON"
    );
}

/// Test overriding form fields for a form body
#[rstest]
#[tokio::test]
async fn test_override_body_form(http_engine: &HttpEngine) {
    let recipe = Recipe {
        // This should implicitly set the content-type header
        body: Some(RecipeBody::FormUrlencoded(indexmap! {
            // Included
            "user_id".into() => "1".into(),
            // Excluded
            "token".into() => "tokenzzz".into(),
            // Overridden
            "preference".into() => "large".into(),
        })),
        ..Recipe::factory(())
    };
    let (context, seed) = template_context(
        recipe,
        [
            (OverrideKey::Form("token".into()), OverrideValue::Omit),
            (OverrideKey::Form("preference".into()), "small".into()),
        ],
    );
    let ticket = http_engine.build(seed, &context).await.unwrap();

    assert_eq!(
        ticket.record.body_str().ok().flatten(),
        Some("user_id=1&preference=small")
    );
}

/// Test launching a built request
#[rstest]
#[tokio::test]
async fn test_send_request(http_engine: &HttpEngine) {
    let host = mock_server().await;
    let recipe = Recipe {
        url: format!("{host}/get").as_str().into(),
        ..Recipe::factory(())
    };
    let (context, seed) = template_context(recipe, []);

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
            "Host".into() => "\n{{host}}\n".into(),
        },
        ..Recipe::factory(())
    };
    let (context, _) = template_context(Recipe::factory(()), []);
    let rendered = recipe.render_headers(&context).await.unwrap();

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
async fn test_build_curl(http_engine: &HttpEngine) {
    let recipe = Recipe {
        method: HttpMethod::Get,
        query: indexmap! {
            "mode".into() => "{{mode}}".into(),
            "fast".into() => ["true", "false"].into(),
        },
        headers: indexmap! {
            "Accept".into() => "application/json".into(),
            "Content-Type".into() => "application/json".into(),
        },
        ..Recipe::factory(())
    };
    let (context, seed) = template_context(recipe, []);

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
        username: "{{username}}".into(),
        password: "{{password}}".into(),
    },
    "--user 'user:hunter2'",
)]
#[case::bearer(
    Authentication::Bearer("{{token}}".into()),
    "--header 'authorization: Bearer tokenzzz'",
)]
#[tokio::test]
async fn test_build_curl_authentication(
    http_engine: &HttpEngine,
    #[case] authentication: Authentication,
    #[case] expected_arguments: &str,
) {
    let recipe = Recipe {
        authentication: Some(authentication),
        ..Recipe::factory(())
    };
    let (context, seed) = template_context(recipe, []);

    let command = http_engine.build_curl(seed, &context).await.unwrap();
    let expected_command = format!(
        "curl -XGET --url 'http://localhost/url' {expected_arguments}",
    );
    assert_eq!(command, expected_command);
}

/// Build a curl command with each possible type of body
#[rstest]
#[case::json(
    RecipeBody::Raw {
        body: json!({"group_id": "{{group_id}}"}).to_string().into(),
        content_type: Some(ContentType::Json),
    },
    "--header 'content-type: application/json' --data '{\"group_id\":\"3\"}'"
)]
#[case::form_urlencoded(
    RecipeBody::FormUrlencoded(indexmap! {
        "user_id".into() => "{{user_id}}".into(),
        "token".into() => "{{token}}".into()
    }),
    "--data-urlencode 'user_id=1' --data-urlencode 'token=tokenzzz'"
)]
#[case::form_multipart(
    // This doesn't support binary content because we can't pass it via cmd
    RecipeBody::FormMultipart(indexmap! {
        "user_id".into() => "{{user_id}}".into(),
        "token".into() => "{{token}}".into()
    }),
    "-F 'user_id=1' -F 'token=tokenzzz'"
)]
#[tokio::test]
async fn test_build_curl_body(
    http_engine: &HttpEngine,
    #[case] body: RecipeBody,
    #[case] expected_arguments: &str,
) {
    let recipe = Recipe {
        body: Some(body),
        ..Recipe::factory(())
    };
    let (context, seed) = template_context(recipe, []);

    let command = http_engine.build_curl(seed, &context).await.unwrap();
    let expected_command =
        format!("curl -XGET --url 'http://localhost/url' {expected_arguments}");
    assert_eq!(command, expected_command);
}

/// Test OverrideKey::FromStr
#[rstest]
#[case::field("field", OverrideKey::Profile("field".into()))]
#[case::url("rq.url", OverrideKey::Url)]
#[case::query_no_index("rq.query.foo", OverrideKey::Query("foo".into(), None))]
#[case::query_index(
    "rq.query.foo.0",
    OverrideKey::Query("foo".into(), Some(0)),
)]
#[case::query_with_dot(
    "rq.query.user.name.0",
    OverrideKey::Query("user.name".into(), Some(0)),
)]
// TODO more
fn test_parse_override_key(#[case] input: &str, #[case] expected: OverrideKey) {
    let actual = input.parse();
    assert_eq!(actual, Ok(expected));
}

/// Test error cases for  OverrideKey::FromStr
#[rstest]
#[case::empty("", "TODO")]
#[case::missing_rq_prefix("url", "TODO")]
#[case::unknown_request_component("rq.unknown", "TODO")]
#[case::bonus_dot("rq.url.", "TODO")]
#[case::dot_in_profile("field.name", "TODO")]
// TODO more
fn test_parse_override_key_error(
    #[case] input: &str,
    #[case] expected_error: &str,
) {
    assert_err!(input.parse::<OverrideKey>(), expected_error);
}

/// By default, the engine will follow 3xx redirects
#[rstest]
#[tokio::test]
async fn test_follow_redirects(http_engine: &HttpEngine) {
    let host = mock_server().await;
    let recipe = Recipe {
        url: format!("{host}/redirect").as_str().into(),
        ..Recipe::factory(())
    };
    let (context, seed) = template_context(recipe, []);

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
    let (context, seed) = template_context(recipe, []);

    // Build+send the request
    let ticket = http_engine.build(seed, &context).await.unwrap();
    let exchange = ticket.send().await.unwrap();

    assert_eq!(exchange.response.status, StatusCode::MOVED_PERMANENTLY);
}
