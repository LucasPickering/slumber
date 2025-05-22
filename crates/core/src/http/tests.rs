//! Tests for the http module

use super::*;
use crate::{
    collection::{Authentication, Chain, ChainSource, Collection, Profile},
    test_util::{
        TestPrompter, by_id, header_map, http_engine, invalid_utf8_chain,
    },
};
use indexmap::{IndexMap, indexmap};
use pretty_assertions::assert_eq;
use regex::Regex;
use reqwest::{Body, StatusCode};
use rstest::rstest;
use serde_json::json;
use slumber_util::Factory;
use std::ptr;
use wiremock::{Mock, MockServer, ResponseTemplate, matchers};

/// Create a template context. Take a set of extra recipes and chains to
/// add to the created collection
fn template_context(
    recipes: impl IntoIterator<Item = Recipe>,
    chains: impl IntoIterator<Item = Chain>,
) -> TemplateContext {
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
    let chains = [Chain {
        id: "text".into(),
        source: ChainSource::Prompt {
            message: None,
            default: None,
        },
        ..Chain::factory(())
    }]
    .into_iter()
    .chain(chains);
    TemplateContext {
        collection: Collection {
            recipes: by_id(recipes).into(),
            profiles: by_id([profile]),
            chains: by_id(chains),
            ..Collection::factory(())
        }
        .into(),
        selected_profile: Some(profile_id.clone()),
        prompter: Box::new(TestPrompter::new(["first", "second"])),
        ..TemplateContext::factory(())
    }
}

/// Create a mock HTTP server and return its URL
async fn mock_server() -> String {
    // Mock HTTP response
    let server = MockServer::start().await;
    let host = server.uri();
    Mock::given(matchers::method("GET"))
        .and(matchers::path("/get"))
        .respond_with(ResponseTemplate::new(200).set_body_string("hello!"))
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
        query: vec![
            ("mode".into(), "{{mode}}".into()),
            ("fast".into(), "true".into()),
        ],
        headers: indexmap! {
            // Leading/trailing newlines should be stripped
            "Accept".into() => "application/json".into(),
            "Content-Type".into() => "application/json".into(),
        },
        body: Some("{\"group_id\":\"{{group_id}}\"}".into()),
        ..Recipe::factory(())
    };
    let recipe_id = recipe.id.clone();
    let template_context = template_context([recipe], []);

    let seed = RequestSeed::new(recipe_id.clone(), BuildOptions::default());
    let ticket = http_engine.build(seed, &template_context).await.unwrap();

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
            profile_id: Some(
                template_context.collection.first_profile_id().clone()
            ),
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
        query: vec![
            ("mode".into(), "{{mode}}".into()),
            ("fast".into(), "true".into()),
            ("fast".into(), "false".into()),
            ("mode".into(), "user".into()),
        ],
        ..Recipe::factory(())
    };
    let recipe_id = recipe.id.clone();
    let template_context = template_context([recipe], []);

    let seed = RequestSeed::new(recipe_id, BuildOptions::default());
    let url = http_engine
        .build_url(seed, &template_context)
        .await
        .unwrap();

    assert_eq!(
        url.as_str(),
        "http://localhost/users/1?mode=sudo&fast=true&fast=false&mode=user"
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
    invalid_utf8_chain: ChainSource,
    #[case] body: RecipeBody,
    #[case] expected_body: &[u8],
) {
    let template_context = template_context(
        [Recipe {
            body: Some(body),
            ..Recipe::factory(())
        }],
        [Chain {
            // Invalid UTF-8
            id: "binary".into(),
            source: invalid_utf8_chain,
            ..Chain::factory(())
        }],
    );
    let seed = RequestSeed::new(
        template_context.collection.first_recipe_id().clone(),
        BuildOptions::default(),
    );
    let body = http_engine
        .build_body(seed, &template_context)
        .await
        .unwrap();

    assert_eq!(body.as_deref(), Some(expected_body));
}

/// Test building requests with various authentication methods
#[rstest]
#[case::basic(
    Authentication::Basic {
        username: "{{username}}".into(),
        password: Some("{{password}}".into()),
    },
    "Basic dXNlcjpodW50ZXIy"
)]
#[case::basic_no_password(
    Authentication::Basic {
        username: "{{username}}".into(),
        password: None,
    },
    "Basic dXNlcjo="
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
    let template_context = template_context([recipe], []);

    let seed = RequestSeed::new(recipe_id.clone(), BuildOptions::default());
    let ticket = http_engine.build(seed, &template_context).await.unwrap();

    assert_eq!(
        *ticket.record,
        RequestRecord {
            id: ticket.record.id,
            profile_id: Some(
                template_context.collection.first_profile_id().clone()
            ),
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
        "binary".into() => "{{chains.binary}}".into()
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
    invalid_utf8_chain: ChainSource,
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
    let template_context = template_context(
        [recipe],
        [Chain {
            // Invalid UTF-8
            id: "binary".into(),
            source: invalid_utf8_chain,
            ..Chain::factory(())
        }],
    );

    let seed = RequestSeed::new(recipe_id.clone(), BuildOptions::default());
    let ticket = http_engine.build(seed, &template_context).await.unwrap();

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
                Some(template_context.collection.first_profile_id().clone()),
                recipe_id
            ))
        }
    );
}

/// Test disabling and overriding authentication, query params, headers, and
/// bodies
#[rstest]
#[tokio::test]
async fn test_build_options(http_engine: &HttpEngine) {
    let recipe = Recipe {
        authentication: Some(Authentication::Basic {
            username: "username".into(),
            password: None,
        }),
        headers: indexmap! {
            // Included
            "Accept".into() => "application/json".into(),
            // Overidden
            "Big-Guy".into() => "style1".into(),
            // Excluded
            "content-type".into() => "text/plain".into(),
        },
        query: vec![
            // Overridden
            ("mode".into(), "regular".into()),
            // Excluded
            ("fast".into(), "false".into()),
            // Included
            ("fast".into(), "true".into()),
        ],
        body: Some(RecipeBody::Raw {
            body: "{{username}}".into(),
            content_type: Some(ContentType::Json),
        }),
        ..Recipe::factory(())
    };
    let recipe_id = recipe.id.clone();
    let template_context = template_context([recipe], []);

    let seed = RequestSeed::new(
        recipe_id.clone(),
        BuildOptions {
            authentication: Some(Authentication::Basic {
                username: "{{username}}".into(),
                password: Some("{{password}}".into()),
            }),
            headers: [
                (1, BuildFieldOverride::Override("style2".into())),
                (2, BuildFieldOverride::Omit),
            ]
            .into_iter()
            .collect(),
            query_parameters: [
                // Overridding template should get rendered
                (0, BuildFieldOverride::Override("{{mode}}".into())),
                (1, BuildFieldOverride::Omit),
            ]
            .into_iter()
            .collect(),
            body: Some("{{password}}".into()),
            // Form field override has to be in a different test, because
            // we're using a raw body
            form_fields: Default::default(),
        },
    );
    let ticket = http_engine.build(seed, &template_context).await.unwrap();

    assert_eq!(
        *ticket.record,
        RequestRecord {
            id: ticket.record.id,
            profile_id: template_context.selected_profile.clone(),
            recipe_id,
            method: HttpMethod::Get,
            http_version: HttpVersion::Http11,
            url: "http://localhost/url?mode=sudo&fast=true".parse().unwrap(),
            headers: header_map([
                ("Authorization", "Basic dXNlcjpodW50ZXIy"),
                ("accept", "application/json"),
                ("Big-Guy", "style2"),
                // It picked up the default content-type from the body,
                // because ours was excluded
                ("content-type", "application/json"),
            ]),
            body: Some(b"hunter2".as_slice().into()),
        }
    );
}

/// Test overriding form body fields. This has to be a separate test
/// because it's incompatible with testing raw body overrides
#[rstest]
#[tokio::test]
async fn test_build_options_form(http_engine: &HttpEngine) {
    let recipe = Recipe {
        // This should implicitly set the content-type header
        body: Some(RecipeBody::FormUrlencoded(indexmap! {
            // Included
            "user_id".into() => "{{user_id}}".into(),
            // Excluded
            "token".into() => "{{token}}".into(),
            // Overridden
            "preference".into() => "large".into(),
        })),
        ..Recipe::factory(())
    };
    let recipe_id = recipe.id.clone();
    let template_context = template_context([recipe], []);

    let seed = RequestSeed::new(
        recipe_id.clone(),
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
    let ticket = http_engine.build(seed, &template_context).await.unwrap();

    assert_eq!(
        *ticket.record,
        RequestRecord {
            id: ticket.record.id,
            profile_id: template_context.selected_profile.clone(),
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

/// Using the same chain in two different templates should be deduplicated,
/// so that the chain is only computed once
#[rstest]
#[tokio::test]
async fn test_chain_duplicate(http_engine: &HttpEngine) {
    let recipe = Recipe {
        method: HttpMethod::Post,
        url: "{{host}}/{{chains.text}}".into(),
        body: Some("{{chains.text}}".into()),
        ..Recipe::factory(())
    };
    let recipe_id = recipe.id.clone();
    let template_context = template_context([recipe], []);

    let seed = RequestSeed::new(recipe_id, BuildOptions::default());
    let ticket = http_engine.build(seed, &template_context).await.unwrap();

    let expected_url: Url = "http://localhost/first".parse().unwrap();
    let expected_body = b"first";

    let request = &ticket.request;
    assert_eq!(request.url(), &expected_url);
    assert_eq!(
        request.body().and_then(Body::as_bytes),
        Some(expected_body.as_slice())
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
    let recipe_id = recipe.id.clone();
    let template_context = template_context([recipe], []);

    // Build+send the request
    let seed = RequestSeed::new(recipe_id, BuildOptions::default());
    let ticket = http_engine.build(seed, &template_context).await.unwrap();
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
    let template_context = template_context([], []);
    let rendered = recipe
        .render_headers(&BuildOptions::default(), &template_context)
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
async fn test_build_curl(http_engine: &HttpEngine) {
    let recipe = Recipe {
        method: HttpMethod::Get,
        query: vec![
            ("mode".into(), "{{mode}}".into()),
            ("fast".into(), "true".into()),
            ("fast".into(), "false".into()),
        ],
        headers: indexmap! {
            "Accept".into() => "application/json".into(),
            "Content-Type".into() => "application/json".into(),
        },
        ..Recipe::factory(())
    };
    let recipe_id = recipe.id.clone();
    let template_context = template_context([recipe], []);

    let seed = RequestSeed::new(recipe_id, BuildOptions::default());
    let command = http_engine
        .build_curl(seed, &template_context)
        .await
        .unwrap();
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
        password: Some("{{password}}".into()),
    },
    "--user 'user:hunter2'",
)]
#[case::basic_no_password(
    Authentication::Basic {
        username: "{{username}}".into(),
        password: None,
    },
    "--user 'user:'",
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
    let recipe_id = recipe.id.clone();
    let template_context = template_context([recipe], []);

    let seed = RequestSeed::new(recipe_id, BuildOptions::default());
    let command = http_engine
        .build_curl(seed, &template_context)
        .await
        .unwrap();
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
    let recipe_id = recipe.id.clone();
    let template_context = template_context([recipe], []);

    let seed = RequestSeed::new(recipe_id.clone(), BuildOptions::default());
    let command = http_engine
        .build_curl(seed, &template_context)
        .await
        .unwrap();
    let expected_command =
        format!("curl -XGET --url 'http://localhost/url' {expected_arguments}");
    assert_eq!(command, expected_command);
}
