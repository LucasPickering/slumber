//! HTTP-specific logic and models. [HttpEngine] is the main entrypoint for all
//! operations. This is the life cycle of a request:
//!
//! +--------+
//! | Recipe |
//! +--------+
//!      |
//!  initialize
//!      |
//!      v
//! +-------------+          +-------------------+
//! | RequestSeed | -error-> | RequestBuildError |
//! +-------------+          +-------------------+
//!      |
//!    build
//!      |
//!      v
//! +---------------+
//! | RequestTicket |
//! +---------------+
//!      |
//!    send
//!      |
//!      v
//! +--------+          +--------------+
//! | future | -error-> | RequestError |
//! +--------+          +--------------+
//!      |
//!   success
//!      |
//!      v
//! +----------+
//! | Exchange |
//! +----------+

mod cereal;
mod content_type;
mod models;
mod query;

pub use content_type::*;
pub use models::*;
pub use query::*;

use crate::{
    collection::{Authentication, JsonBody, Method, Recipe, RecipeBody},
    config::Config,
    db::CollectionDatabase,
    template::{Template, TemplateContext},
    util::ResultExt,
};
use anyhow::Context;
use async_recursion::async_recursion;
use bytes::Bytes;
use chrono::Utc;
use futures::{
    future::{self, try_join_all, OptionFuture},
    Future,
};
use reqwest::{
    header::{self, HeaderMap, HeaderName, HeaderValue},
    Client, RequestBuilder, Response, Url,
};
use std::{collections::HashSet, sync::Arc};
use tokio::try_join;
use tracing::{info, info_span};

const USER_AGENT: &str =
    concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));

/// Utility for handling all HTTP operations. The main purpose of this is to
/// de-asyncify HTTP so it can be called in the main TUI thread. All heavy
/// lifting will be pushed to background tasks.
///
/// This is safe and cheap to clone because reqwest's `Client` type uses `Arc`
/// internally. [reqwest::Client]
#[derive(Clone, Debug)]
pub struct HttpEngine {
    client: Client,
    /// This client ignores TLS cert errors. Only use it if the user
    /// specifically wants to ignore errors for the request!
    danger_client: Client,
    /// Hostnames for which we should ignore TLS
    danger_hostnames: HashSet<String>,
}

impl HttpEngine {
    /// Build a new HTTP engine, which can be used for the entire program life
    pub fn new(config: &Config) -> Self {
        Self {
            client: Client::builder()
                .user_agent(USER_AGENT)
                .build()
                .expect("Error building reqwest client"),
            danger_client: Client::builder()
                .user_agent(USER_AGENT)
                .danger_accept_invalid_certs(true)
                .build()
                .expect("Error building reqwest client"),
            danger_hostnames: config
                .ignore_certificate_hosts
                .iter()
                .cloned()
                .collect(),
        }
    }

    /// Build a [RequestTicket] from a [RequestSeed]. This will render the
    /// recipe into a request. The returned ticket can then be launched.
    pub async fn build(
        &self,
        seed: RequestSeed,
        template_context: &TemplateContext,
    ) -> Result<RequestTicket, RequestBuildError> {
        let RequestSeed {
            id,
            recipe,
            options,
        } = &seed;
        let _ =
            info_span!("Build request", request_id = %id, ?recipe, ?options)
                .entered();

        let future = async {
            // Render everything up front so we can parallelize it
            let (url, query, headers, authentication, body) = try_join!(
                recipe.render_url(template_context),
                recipe.render_query(options, template_context),
                recipe.render_headers(options, template_context),
                recipe.render_authentication(template_context),
                recipe.render_body(options, template_context),
            )?;

            // Build the reqwest request first, so we can have it do all the
            // hard work of encoding query params/authorization/etc.
            // We'll just copy its homework at the end to get our
            // RequestRecord
            let client = self.get_client(&url);
            let mut builder =
                client.request(recipe.method.into(), url).query(&query);
            if let Some(body) = body {
                builder = body.apply(builder);
            }
            // Set headers *after* body so the use can override the Content-Type
            // header that was set if they want to
            builder = builder.headers(headers);
            if let Some(authentication) = authentication {
                builder = authentication.apply(builder);
            }

            let request = builder.build()?;
            Ok((client, request))
        };
        let (client, request) =
            seed.convert_error(future, template_context).await?;

        Ok(RequestTicket {
            record: RequestRecord::new(
                seed,
                template_context.selected_profile.clone(),
                &request,
            )
            .into(),
            client: client.clone(),
            request,
        })
    }

    /// Render *just* the URL of a request, including query parameters
    pub async fn build_url(
        &self,
        seed: RequestSeed,
        template_context: &TemplateContext,
    ) -> Result<Url, RequestBuildError> {
        let RequestSeed {
            id,
            recipe,
            options,
        } = &seed;
        let _ =
            info_span!("Build request URL", request_id = %id, ?recipe, ?options)
                .entered();

        let future = async {
            // Parallelization!
            let (url, query) = try_join!(
                recipe.render_url(template_context),
                recipe.render_query(options, template_context),
            )?;

            // Use RequestBuilder so we can offload the handling of query params
            let client = self.get_client(&url);
            let request = client
                .request(recipe.method.into(), url)
                .query(&query)
                .build()?;
            Ok(request)
        };
        let request = seed.convert_error(future, template_context).await?;

        Ok(request.url().clone())
    }

    /// Render *just* the body of a request
    pub async fn build_body(
        &self,
        seed: RequestSeed,
        template_context: &TemplateContext,
    ) -> Result<Option<Bytes>, RequestBuildError> {
        let RequestSeed {
            id,
            recipe,
            options,
        } = &seed;
        let _ =
            info_span!("Build request body", request_id = %id, ?recipe, ?options)
                .entered();

        let future = async {
            let Some(body) =
                recipe.render_body(options, template_context).await?
            else {
                return Ok(None);
            };

            match body {
                // If we have the bytes, we don't need to bother building a
                // request
                RenderedBody::Raw(bytes) => Ok(Some(bytes)),
                // The body is complex - offload the hard work to RequestBuilder
                RenderedBody::FormUrlencoded(_) => {
                    let url = Url::parse("http://localhost").unwrap();
                    let client = self.get_client(&url);
                    let mut builder = client.request(reqwest::Method::GET, url);
                    builder = body.apply(builder);
                    let request = builder.build()?;
                    // We just added a body so we know it's present, and we
                    // know it's not a stream. This requires a clone which sucks
                    // because the bytes are going to get thrown away anyway,
                    // but nothing we can do about that because of reqwest's API
                    let bytes = request
                        .body()
                        .expect("Body should be present")
                        .as_bytes()
                        .expect("Body should be raw bytes")
                        .to_owned()
                        .into();
                    Ok(Some(bytes))
                }
            }
        };
        seed.convert_error(future, template_context).await
    }

    /// Get the appropriate client to use for this request. If the request URL's
    /// host is one for which the user wants to ignore TLS certs, use the
    /// dangerous client.
    fn get_client(&self, url: &Url) -> &Client {
        let host = url.host_str().unwrap_or_default();
        if self.danger_hostnames.contains(host) {
            &self.danger_client
        } else {
            &self.client
        }
    }
}

impl RequestSeed {
    /// Run the given future and convert any error into [RequestBuildError]
    async fn convert_error<T>(
        &self,
        future: impl Future<Output = anyhow::Result<T>>,
        template_context: &TemplateContext,
    ) -> Result<T, RequestBuildError> {
        future.await.traced().map_err(|error| RequestBuildError {
            profile_id: template_context.selected_profile.clone(),
            recipe_id: self.recipe.id.clone(),
            id: self.id,
            time: Utc::now(),
            error,
        })
    }
}

impl RequestTicket {
    /// Launch an HTTP request. Upon completion, it will automatically be
    /// registered in the database for posterity.
    ///
    /// Returns a full HTTP exchange, which includes the originating request,
    /// the response, and the start/end timestamps. We can't report a reliable
    /// start time until after the future is resolved, because the request isn't
    /// launched until the consumer starts awaiting the future. For in-flight
    /// time tracking, track your own start time immediately before/after
    /// sending the request.
    pub async fn send(
        self,
        database: &CollectionDatabase,
    ) -> Result<Exchange, RequestError> {
        let id = self.record.id;

        // Capture the rest of this method in a span
        let _ = info_span!("HTTP request", request_id = %id).entered();

        // This start time will be accurate because the request doesn't launch
        // until this whole future is awaited
        let start_time = Utc::now();
        let result = async {
            let response = self.client.execute(self.request).await?;
            // Load the full response and convert it to our format
            ResponseRecord::from_response(response).await
        }
        .await;
        let end_time = Utc::now();

        match result {
            Ok(response) => {
                info!(status = response.status.as_u16(), "Response");
                let exchange = Exchange {
                    id,
                    request: self.record,
                    response: Arc::new(response),
                    start_time,
                    end_time,
                };

                // Error here should *not* kill the request
                let _ = database.insert_exchange(&exchange);
                Ok(exchange)
            }

            // Attach metadata to the error and yeet it. Can't use map_err
            // because we need to conditionally move the request
            Err(error) => Err(RequestError {
                request: self.record,
                start_time,
                end_time,
                error: error.into(),
            })
            .traced(),
        }
    }
}

impl ResponseRecord {
    /// Convert [reqwest::Response] type into [ResponseRecord]. This is async
    /// because the response content is not necessarily loaded when we first get
    /// the response. Only fails if the response content fails to load.
    async fn from_response(
        response: Response,
    ) -> reqwest::Result<ResponseRecord> {
        // Copy response metadata out first, because we need to move the
        // response to resolve content (not sure why...)
        let status = response.status();
        let headers = response.headers().clone();

        // Pre-resolve the content, so we get all the async work done
        let body = response.bytes().await?.into();

        Ok(ResponseRecord {
            status,
            headers,
            body,
        })
    }
}

/// Render steps for individual pieces of a recipe
impl Recipe {
    /// Render base URL, *excluding* query params
    async fn render_url(
        &self,
        template_context: &TemplateContext,
    ) -> anyhow::Result<Url> {
        let url = self
            .url
            .render_string(template_context)
            .await
            .context("Error rendering URL")?;
        url.parse::<Url>()
            .with_context(|| format!("Invalid URL: `{url}`"))
    }

    /// Render query key=value params
    async fn render_query(
        &self,
        options: &BuildOptions,
        template_context: &TemplateContext,
    ) -> anyhow::Result<Vec<(String, String)>> {
        let iter = self
            .query
            .iter()
            .enumerate()
            // Filter out disabled params. We do this by index because the keys
            // aren't necessarily unique
            .filter(|(i, _)| !options.disabled_query_parameters.contains(i))
            .map(|(_, (k, v))| async move {
                Ok::<_, anyhow::Error>((
                    k.clone(),
                    v.render_string(template_context).await.context(
                        format!("Error rendering query parameter `{k}`"),
                    )?,
                ))
            });
        future::try_join_all(iter).await
    }

    /// Render all headers specified by the user. This will *not* include
    /// authentication and other implicit headers
    async fn render_headers(
        &self,
        options: &BuildOptions,
        template_context: &TemplateContext,
    ) -> anyhow::Result<HeaderMap> {
        let mut headers = HeaderMap::new();

        // Set Content-Type based on the body type. This can be overwritten
        // below if the user explicitly passed a Content-Type value
        if let Some(content_type) =
            self.body.as_ref().and_then(|body| body.mime())
        {
            headers.insert(
                header::CONTENT_TYPE,
                content_type
                    .as_ref()
                    // A MIME type should always be a valid header value
                    .try_into()
                    .expect("Invalid MIME"),
            );
        }

        // Render headers in an iterator so we can parallelize
        let iter = self
            .headers
            .iter()
            .enumerate()
            // Filter out disabled headers
            .filter(|(i, _)| !options.disabled_headers.contains(i))
            .map(move |(_, (header, value_template))| {
                self.render_header(template_context, header, value_template)
            });

        let rendered = future::try_join_all(iter).await?;
        headers.reserve(rendered.len());
        // Do *not* use headers.extend(), because that will append to existing
        // headers, and we want to overwrite instead
        for (header, value) in rendered {
            headers.insert(header, value);
        }

        Ok(headers)
    }

    /// Render a single key/value header
    async fn render_header(
        &self,
        template_context: &TemplateContext,
        header: &str,
        value_template: &Template,
    ) -> anyhow::Result<(HeaderName, HeaderValue)> {
        let mut value = value_template
            .render(template_context)
            .await
            .context(format!("Error rendering header `{header}`"))?;

        // Strip leading/trailing line breaks because they're going to trigger a
        // validation error and are probably a mistake. We're trading
        // explicitness for convenience here. This is maybe redundant now with
        // the Chain::trim field, but this behavior predates that field so it's
        // left in for backward compatibility.
        trim_bytes(&mut value, |c| c == b'\n' || c == b'\r');

        // String -> header conversions are fallible, if headers
        // are invalid
        Ok::<(HeaderName, HeaderValue), anyhow::Error>((
            header
                .try_into()
                .context(format!("Error encoding header name `{header}`"))?,
            value.try_into().context(format!(
                "Error encoding value for header `{header}`"
            ))?,
        ))
    }

    /// Render authentication and return the same data structure, with resolved
    /// data. This can be passed to [reqwest::RequestBuilder]
    async fn render_authentication(
        &self,
        template_context: &TemplateContext,
    ) -> anyhow::Result<Option<Authentication<String>>> {
        match &self.authentication {
            Some(Authentication::Basic { username, password }) => {
                let (username, password) = try_join!(
                    async {
                        username
                            .render_string(template_context)
                            .await
                            .context("Error rendering username")
                    },
                    async {
                        OptionFuture::from(password.as_ref().map(|password| {
                            password.render_string(template_context)
                        }))
                        .await
                        .transpose()
                        .context("Error rendering password")
                    },
                )?;
                Ok(Some(Authentication::Basic { username, password }))
            }

            Some(Authentication::Bearer(token)) => {
                let token = token
                    .render_string(template_context)
                    .await
                    .context("Error rendering bearer token")?;
                Ok(Some(Authentication::Bearer(token)))
            }
            None => Ok(None),
        }
    }

    /// Render request body
    async fn render_body(
        &self,
        options: &BuildOptions,
        template_context: &TemplateContext,
    ) -> anyhow::Result<Option<RenderedBody>> {
        let Some(body) = &self.body else {
            return Ok(None);
        };

        let rendered = match body {
            RecipeBody::Raw(body) => RenderedBody::Raw(
                body.render(template_context)
                    .await
                    .context("Error rendering body")?
                    .into(),
            ),
            // Recursively render the JSON body
            RecipeBody::Json(value) => RenderedBody::Raw(
                value
                    .render(template_context)
                    .await
                    .context("Error rendering body")?
                    .to_string()
                    .into(),
            ),
            RecipeBody::FormUrlencoded(fields) => {
                let iter = fields
                    .iter()
                    .enumerate()
                    // Remove disabled fields
                    .filter(|(i, _)| !options.disabled_form_fields.contains(i))
                    .map(|(_, (k, v))| async move {
                        Ok::<_, anyhow::Error>((
                            k.clone(),
                            v.render_string(template_context).await.context(
                                format!("Error rendering form field `{k}`"),
                            )?,
                        ))
                    });
                let rendered = try_join_all(iter).await?;
                RenderedBody::FormUrlencoded(rendered)
            }
        };
        Ok(Some(rendered))
    }
}

impl Authentication<String> {
    fn apply(self, builder: RequestBuilder) -> RequestBuilder {
        match self {
            Authentication::Basic { username, password } => {
                builder.basic_auth(username, password)
            }
            Authentication::Bearer(token) => builder.bearer_auth(token),
        }
    }
}

impl JsonBody {
    /// Recursively render the JSON value. All string values will be rendered
    /// as templates; other primitives remain the same.
    #[async_recursion]
    async fn render(
        &self,
        template_context: &TemplateContext,
    ) -> anyhow::Result<serde_json::Value> {
        let rendered = match self {
            JsonBody::Null => serde_json::Value::Null,
            JsonBody::Bool(b) => serde_json::Value::Bool(*b),
            JsonBody::Number(n) => serde_json::Value::Number(n.clone()),
            JsonBody::String(template) => serde_json::Value::String(
                template.render_string(template_context).await?,
            ),
            JsonBody::Array(values) => serde_json::Value::Array(
                try_join_all(
                    values.iter().map(|value| value.render(template_context)),
                )
                .await?
                .into_iter()
                .collect(),
            ),
            JsonBody::Object(items) => serde_json::Value::Object(
                try_join_all(items.iter().map(|(key, value)| async {
                    Ok::<_, anyhow::Error>((
                        key.clone(),
                        value.render(template_context).await?,
                    ))
                }))
                .await?
                .into_iter()
                .collect(),
            ),
        };
        Ok(rendered)
    }
}

/// Body ready to be added to the request. Each variant corresponds to a method
/// by which we'll add it to the request. This means it is **not** 1:1 with
/// [RecipeBody]
enum RenderedBody {
    Raw(Bytes),
    /// Value is `String` because only string data can be URL-encoded
    FormUrlencoded(Vec<(String, String)>),
}

impl RenderedBody {
    fn apply(self, builder: RequestBuilder) -> RequestBuilder {
        // Set body. The variant tells us _how_ to set it
        match self {
            RenderedBody::Raw(bytes) => builder.body(bytes),
            RenderedBody::FormUrlencoded(fields) => builder.form(&fields),
        }
    }
}

impl From<Method> for reqwest::Method {
    fn from(method: Method) -> Self {
        match method {
            Method::Connect => reqwest::Method::CONNECT,
            Method::Delete => reqwest::Method::DELETE,
            Method::Get => reqwest::Method::GET,
            Method::Head => reqwest::Method::HEAD,
            Method::Options => reqwest::Method::OPTIONS,
            Method::Patch => reqwest::Method::PATCH,
            Method::Post => reqwest::Method::POST,
            Method::Put => reqwest::Method::PUT,
            Method::Trace => reqwest::Method::TRACE,
        }
    }
}

/// Trim the bytes from the beginning and end of a vector that match the given
/// predicate. This will mutate the input vector. If bytes are trimmed off the
/// start, it will be done with a single shift.
fn trim_bytes(bytes: &mut Vec<u8>, f: impl Fn(u8) -> bool) {
    // Trim start
    for i in 0..bytes.len() {
        if !f(bytes[i]) {
            bytes.drain(0..i);
            break;
        }
    }

    // Trim end
    for i in (0..bytes.len()).rev() {
        if !f(bytes[i]) {
            bytes.drain((i + 1)..bytes.len());
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        collection::{
            self, Authentication, Chain, ChainSource, Collection, Profile,
        },
        test_util::{by_id, header_map, Factory},
    };
    use indexmap::{indexmap, IndexMap};
    use pretty_assertions::assert_eq;
    use reqwest::{Method, StatusCode};
    use rstest::{fixture, rstest};
    use serde_json::json;

    #[fixture]
    fn http_engine() -> HttpEngine {
        HttpEngine::new(&Config::default())
    }

    #[fixture]
    fn template_context() -> TemplateContext {
        let profile_data = indexmap! {
            "host".into() => "http://localhost".into(),
            "mode".into() => "sudo".into(),
            "user_id".into() => "1".into(),
            "group_id".into() => "3".into(),
            "token".into() => "hunter2".into(),
        };
        let profile = Profile {
            data: profile_data,
            ..Profile::factory(())
        };
        let profile_id = profile.id.clone();
        let binary_chain = Chain {
            // Invalid UTF-8
            id: "binary".into(),
            source: ChainSource::command(["echo", "-n", "-e", r#"\xc3\x28"#]),
            ..Chain::factory(())
        };
        TemplateContext {
            collection: Collection {
                profiles: by_id([profile]),
                chains: by_id([binary_chain]),
                ..Collection::factory(())
            },
            selected_profile: Some(profile_id.clone()),
            ..TemplateContext::factory(())
        }
    }

    #[rstest]
    #[tokio::test]
    async fn test_build_request(
        http_engine: HttpEngine,
        template_context: TemplateContext,
    ) {
        let recipe = Recipe {
            method: collection::Method::Post,
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

        let seed = RequestSeed::new(recipe, BuildOptions::default());
        let ticket = http_engine.build(seed, &template_context).await.unwrap();

        assert_eq!(
            *ticket.record,
            RequestRecord {
                id: ticket.record.id,
                profile_id: Some(
                    template_context.collection.first_profile_id().clone()
                ),
                recipe_id,
                method: Method::POST,
                url: "http://localhost/users/1?mode=sudo&fast=true"
                    .parse()
                    .unwrap(),
                body: Some(Vec::from(b"{\"group_id\":\"3\"}").into()),
                headers: header_map([
                    ("content-type", "application/json"),
                    ("accept", "application/json"),
                ]),
            }
        );
    }

    /// Test building just a URL. Should include query params, but headers/body
    /// should *not* be built
    #[rstest]
    #[tokio::test]
    async fn test_build_url(
        http_engine: HttpEngine,
        template_context: TemplateContext,
    ) {
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

        let seed = RequestSeed::new(recipe, BuildOptions::default());
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
        RecipeBody::Raw(r#"{"group_id":"{{group_id}}"}"#.into()),
        br#"{"group_id":"3"}"#
    )]
    #[case::json(
        RecipeBody::Json(json!({"group_id": "{{group_id}}"}).into()),
        br#"{"group_id":"3"}"#,
    )]
    #[case::binary(RecipeBody::Raw("{{chains.binary}}".into()), b"\xc3\x28")]
    #[tokio::test]
    async fn test_build_body(
        http_engine: HttpEngine,
        template_context: TemplateContext,
        #[case] body: RecipeBody,
        #[case] expected_body: &[u8],
    ) {
        let recipe = Recipe {
            body: Some(body),
            ..Recipe::factory(())
        };

        let seed = RequestSeed::new(recipe, BuildOptions::default());
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
    #[case::bearer(Authentication::Bearer("{{token}}".into()), "Bearer token!")]
    #[tokio::test]
    async fn test_authentication(
        http_engine: HttpEngine,
        #[case] authentication: Authentication,
        #[case] expected_header: &str,
    ) {
        let profile_data = indexmap! {
            "username".into() => "user".into(),
            "password".into() => "hunter2".into(),
            "token".into() => "token!".into(),
        };
        let profile = Profile {
            data: profile_data,
            ..Profile::factory(())
        };
        let profile_id = profile.id.clone();
        let template_context = TemplateContext {
            collection: Collection {
                profiles: by_id([profile]),
                ..Collection::factory(())
            },
            selected_profile: Some(profile_id.clone()),
            ..TemplateContext::factory(())
        };
        let recipe = Recipe {
            // `Authorization` header should appear twice. This probably isn't
            // something a user would ever want to do, but it should be
            // well-defined
            headers: indexmap! {"Authorization".into() => "bogus".into()},
            authentication: Some(authentication),
            ..Recipe::factory(())
        };
        let recipe_id = recipe.id.clone();

        let seed = RequestSeed::new(recipe, BuildOptions::default());
        let ticket = http_engine.build(seed, &template_context).await.unwrap();

        assert_eq!(
            *ticket.record,
            RequestRecord {
                id: ticket.record.id,
                profile_id: Some(profile_id),
                recipe_id,
                method: Method::GET,
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
    /// we need this to test that the `content-type` header is set correctly
    #[rstest]
    #[case::json(
        RecipeBody::Json(json!({"group_id": "{{group_id}}"}).into()),
        None,
        br#"{"group_id":"3"}"#,
        "application/json"
    )]
    // Content-Type has been overridden by an explicit header
    #[case::json_content_type_override(
        RecipeBody::Json(json!({"group_id": "{{group_id}}"}).into()),
        Some("text/plain"),
        br#"{"group_id":"3"}"#,
        "text/plain"
    )]
    #[case::form_urlencoded(
        RecipeBody::FormUrlencoded(indexmap! {
            "user_id".into() => "{{user_id}}".into(),
            "token".into() => "{{token}}".into()
        }),
        None,
        b"user_id=1&token=hunter2",
        "application/x-www-form-urlencoded"
    )]
    // reqwest sets the content type when initializing the body, so make sure
    // that doesn't override the user's value
    #[case::form_urlencoded_content_type_override(
        RecipeBody::FormUrlencoded(Default::default()),
        Some("text/plain"),
        b"",
        "text/plain"
    )]
    #[tokio::test]
    async fn test_structured_body(
        http_engine: HttpEngine,
        template_context: TemplateContext,
        #[case] body: RecipeBody,
        #[case] content_type: Option<&str>,
        #[case] expected_body: &'static [u8],
        #[case] expected_content_type: &str,
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

        let seed = RequestSeed::new(recipe, BuildOptions::default());
        let ticket = http_engine.build(seed, &template_context).await.unwrap();

        assert_eq!(
            *ticket.record,
            RequestRecord {
                id: ticket.record.id,
                body: Some(expected_body.into()),
                headers: header_map([("content-type", expected_content_type)]),
                ..RequestRecord::factory((
                    Some(
                        template_context.collection.first_profile_id().clone()
                    ),
                    recipe_id
                ))
            }
        );
    }

    /// Test disabling query params, headers, and form fields
    #[rstest]
    #[tokio::test]
    async fn test_build_options(
        http_engine: HttpEngine,
        template_context: TemplateContext,
    ) {
        let recipe = Recipe {
            query: vec![
                // Included
                ("mode".into(), "sudo".into()),
                ("fast".into(), "false".into()),
                // Excluded
                ("fast".into(), "true".into()),
            ],
            headers: indexmap! {
                // Included
                "Accept".into() => "application/json".into(),
                // Excluded
                "content-type".into() => "text/plain".into(),
            },
            // This should implicitly set the content-type header
            body: Some(RecipeBody::FormUrlencoded(indexmap! {
                "user_id".into() => "{{user_id}}".into(),
                "token".into() => "{{token}}".into(),
            })),
            ..Recipe::factory(())
        };
        let recipe_id = recipe.id.clone();

        let seed = RequestSeed::new(
            recipe,
            BuildOptions {
                disabled_headers: vec![1],
                disabled_query_parameters: vec![2],
                disabled_form_fields: vec![1],
            },
        );
        let ticket = http_engine.build(seed, &template_context).await.unwrap();

        assert_eq!(
            *ticket.record,
            RequestRecord {
                id: ticket.record.id,
                profile_id: template_context.selected_profile.clone(),
                recipe_id,
                method: Method::GET,
                url: "http://localhost/url?mode=sudo&fast=false"
                    .parse()
                    .unwrap(),
                headers: header_map([
                    ("accept", "application/json"),
                    ("content-type", "application/x-www-form-urlencoded"),
                ]),
                body: Some(b"user_id=1".as_slice().into()),
            }
        );
    }

    /// Test launching a built request
    #[rstest]
    #[tokio::test]
    async fn test_send_request(
        http_engine: HttpEngine,
        template_context: TemplateContext,
    ) {
        // Mock HTTP response
        let mut server = mockito::Server::new_async().await;
        let url = server.url();
        let mock = server
            .mock("GET", "/get")
            .with_status(200)
            .with_body("hello!")
            .create_async()
            .await;

        let recipe = Recipe {
            url: format!("{url}/get").as_str().into(),
            ..Recipe::factory(())
        };

        // Build+send the request
        let seed = RequestSeed::new(recipe, BuildOptions::default());
        let ticket = http_engine.build(seed, &template_context).await.unwrap();
        let exchange = ticket.send(&template_context.database).await.unwrap();

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
                status: StatusCode::OK,
                headers: header_map([
                    ("connection", "close"),
                    ("content-length", "6"),
                    ("date", date_header),
                ]),
                body: ResponseBody::new(b"hello!".as_slice().into())
            }
        );

        mock.assert();
    }

    /// Leading/trailing newlines should be stripped from rendered header
    /// values. These characters are invalid and trigger an error, so we assume
    /// they're unintentional and the user won't miss them.
    #[rstest]
    #[tokio::test]
    async fn test_render_headers_strip(template_context: TemplateContext) {
        let recipe = Recipe {
            // Leading/trailing newlines should be stripped
            headers: indexmap! {
                "Accept".into() => "application/json".into(),
                "Host".into() => "\n{{host}}\n".into(),
            },
            ..Recipe::factory(())
        };
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
}
