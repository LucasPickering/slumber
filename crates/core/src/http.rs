//! HTTP-specific logic and models. [HttpEngine] is the main entrypoint for all
//! operations. This is the life cycle of a request:
//!
//! ```no_test
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
//! +--------+           +--------------+
//! | future | +-error-> | RequestError |
//! +--------+ |         +--------------+
//!      |     |
//!   success  |         +-------------+
//!      |     +cancel-> | <cancelled> |
//!      v               +-------------+
//! +----------+
//! | Exchange |
//! +----------+
//! ```

pub mod content_type;
mod curl;
mod models;
#[cfg(test)]
mod tests;

pub use models::*;

use crate::{
    collection::{Authentication, Recipe, RecipeBody},
    http::curl::CurlBuilder,
    render::{FromRendered, OverrideKey, OverrideValue, Procedure, Renderer},
};
use anyhow::{Context, bail};
use bytes::Bytes;
use chrono::Utc;
use futures::{Future, future::try_join_all, try_join};
use petitscript::Value;
use reqwest::{
    Client, RequestBuilder, Response, Url,
    header::{HeaderMap, HeaderName, HeaderValue},
    multipart::{Form, Part},
};
use slumber_config::HttpEngineConfig;
use slumber_util::ResultTraced;
use std::{collections::HashSet, error::Error};
use tracing::{error, info, info_span};

const USER_AGENT: &str = concat!("slumber/", env!("CARGO_PKG_VERSION"));

/// Utility for handling all HTTP operations. The main purpose of this is to
/// de-asyncify HTTP so it can be called in the main TUI thread. All heavy
/// lifting will be pushed to background tasks.
///
/// This is safe and cheap to clone because reqwest's `Client` type uses `Arc`
/// internally. [reqwest::Client]
#[derive(Clone, Debug)]
pub struct HttpEngine {
    client: Client,
    /// A client that ignores TLS errors, and the hostnames we should use it
    /// for. If the user didn't specify any (99.9% of cases), don't bother
    /// creating a client because it's expensive.
    danger_client: Option<(Client, HashSet<String>)>,
    large_body_size: usize,
}

impl HttpEngine {
    /// Build a new HTTP engine, which can be used for the entire program life
    pub fn new(config: &HttpEngineConfig) -> Self {
        let client = Client::builder()
            .user_agent(USER_AGENT)
            .build()
            .expect("Error building reqwest client");
        let danger_client = if config.ignore_certificate_hosts.is_empty() {
            None
        } else {
            Some((
                Client::builder()
                    .user_agent(USER_AGENT)
                    .danger_accept_invalid_certs(true)
                    .build()
                    .expect("Error building reqwest client"),
                config.ignore_certificate_hosts.iter().cloned().collect(),
            ))
        };
        Self {
            client,
            danger_client,
            large_body_size: config.large_body_size,
        }
    }

    /// Build a [RequestTicket] from a [RequestSeed]. This will render the
    /// recipe into a request. The returned ticket can then be launched.
    pub async fn build(
        &self,
        seed: RequestSeed,
        renderer: &Renderer,
    ) -> Result<RequestTicket, RequestBuildError> {
        let RequestSeed { id, recipe_id } = &seed;
        let _ =
            info_span!("Build request", request_id = %id, ?recipe_id).entered();

        let future = async {
            let recipe = renderer
                .context()
                .collection
                .recipes
                .try_get_recipe(recipe_id)?;

            // Render everything up front so we can parallelize it
            let (url, query, headers, authentication, body) = try_join!(
                recipe.render_url(renderer),
                recipe.render_query(renderer),
                recipe.render_headers(renderer),
                recipe.render_authentication(renderer),
                recipe.render_body(renderer),
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
        let (client, request) = seed.run_future(future, renderer).await?;

        Ok(RequestTicket {
            record: RequestRecord::new(
                seed,
                renderer.context().selected_profile.clone(),
                &request,
                self.large_body_size,
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
        renderer: &Renderer,
    ) -> Result<Url, RequestBuildError> {
        let RequestSeed { id, recipe_id } = &seed;
        let _ = info_span!("Build request URL", request_id = %id, ?recipe_id)
            .entered();

        let future = async {
            let recipe = renderer
                .context()
                .collection
                .recipes
                .try_get_recipe(recipe_id)?;

            // Parallelization!
            let (url, query) = try_join!(
                recipe.render_url(renderer),
                recipe.render_query(renderer),
            )?;

            // Use RequestBuilder so we can offload the handling of query params
            let client = self.get_client(&url);
            let request = client
                .request(recipe.method.into(), url)
                .query(&query)
                .build()?;
            Ok(request)
        };
        let request = seed.run_future(future, renderer).await?;

        Ok(request.url().clone())
    }

    /// Render *just* the body of a request
    pub async fn build_body(
        &self,
        seed: RequestSeed,
        renderer: &Renderer,
    ) -> Result<Option<Bytes>, RequestBuildError> {
        let RequestSeed { id, recipe_id } = &seed;
        let _ = info_span!("Build request body", request_id = %id, ?recipe_id)
            .entered();

        let future = async {
            let recipe = renderer
                .context()
                .collection
                .recipes
                .try_get_recipe(recipe_id)?;

            let Some(body) = recipe.render_body(renderer).await? else {
                return Ok(None);
            };

            match body {
                // If we have the bytes, we don't need to bother building a
                // request
                RenderedBody::Raw(bytes) => Ok(Some(bytes)),
                // The body is complex - offload the hard work to RequestBuilder
                RenderedBody::Json(_)
                | RenderedBody::FormUrlencoded(_)
                | RenderedBody::FormMultipart(_) => {
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
        seed.run_future(future, renderer).await
    }

    /// Render a recipe into a cURL command that will execute the request.
    ///
    /// Only fails if a header value or body is binary. We can't represent
    /// binary values in the command, so we'd have to push them to a temp file
    /// and have curl extract from there. It's possible, I just haven't done it
    /// yet.
    pub async fn build_curl(
        &self,
        seed: RequestSeed,
        renderer: &Renderer,
    ) -> Result<String, RequestBuildError> {
        let RequestSeed { id, recipe_id } = &seed;
        let _ = info_span!("Build request cURL", request_id = %id, ?recipe_id)
            .entered();

        let future = async {
            let recipe = renderer
                .context()
                .collection
                .recipes
                .try_get_recipe(recipe_id)?;

            // Render everything up front so we can parallelize it
            let (url, query, headers, authentication, body) = try_join!(
                recipe.render_url(renderer),
                recipe.render_query(renderer),
                recipe.render_headers(renderer),
                recipe.render_authentication(renderer),
                recipe.render_body(renderer),
            )?;

            // Buidl the command
            let mut builder = CurlBuilder::new(recipe.method)
                .url(url, &query)
                .headers(&headers)?;
            if let Some(authentication) = authentication {
                builder = builder.authentication(&authentication);
            }
            if let Some(body) = body {
                builder = builder.body(&body)?;
            }
            Ok(builder.build())
        };
        seed.run_future(future, renderer).await
    }

    /// Get the appropriate client to use for this request. If the request URL's
    /// host is one for which the user wants to ignore TLS certs, use the
    /// dangerous client.
    fn get_client(&self, url: &Url) -> &Client {
        let host = url.host_str().unwrap_or_default();
        match &self.danger_client {
            Some((client, hostnames)) if hostnames.contains(host) => client,
            _ => &self.client,
        }
    }
}

impl Default for HttpEngine {
    fn default() -> Self {
        Self::new(&HttpEngineConfig::default())
    }
}

impl RequestSeed {
    /// Run the given future and convert any error into [RequestBuildError]
    async fn run_future<T>(
        &self,
        future: impl Future<Output = anyhow::Result<T>>,
        renderer: &Renderer,
    ) -> Result<T, RequestBuildError> {
        let start_time = Utc::now();
        future.await.traced().map_err(|error| RequestBuildError {
            profile_id: renderer.context().selected_profile.clone(),
            recipe_id: self.recipe_id.clone(),
            id: self.id,
            start_time,
            end_time: Utc::now(),
            source: error,
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
    pub async fn send(self) -> Result<Exchange, RequestError> {
        let id = self.record.id;

        // Capture the rest of this method in a span
        let _ = info_span!("HTTP request", request_id = %id).entered();

        // This start time will be accurate because the request doesn't launch
        // until this whole future is awaited
        let start_time = Utc::now();
        let result = async {
            let response = self.client.execute(self.request).await?;
            // Load the full response and convert it to our format
            ResponseRecord::from_response(id, response).await
        }
        .await;
        let end_time = Utc::now();

        match result {
            Ok(response) => {
                info!(status = response.status.as_u16(), "Response");
                let exchange = Exchange {
                    id,
                    request: self.record,
                    response: response.into(),
                    start_time,
                    end_time,
                };

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
            .inspect_err(|err| error!(error = err as &dyn Error)),
        }
    }
}

impl ResponseRecord {
    /// Convert [reqwest::Response] type into [ResponseRecord]. This is async
    /// because the response content is not necessarily loaded when we first get
    /// the response. Only fails if the response content fails to load.
    async fn from_response(
        id: RequestId,
        response: Response,
    ) -> reqwest::Result<ResponseRecord> {
        // Copy response metadata out first, because we need to move the
        // response to resolve content (not sure why...)
        let status = response.status();
        let headers = response.headers().clone();

        // Pre-resolve the content, so we get all the async work done
        let body = response.bytes().await?.into();

        Ok(ResponseRecord {
            id,
            status,
            headers,
            body,
        })
    }
}

/// Render steps for individual pieces of a recipe
impl Recipe {
    /// Render base URL, *excluding* query params
    async fn render_url(&self, renderer: &Renderer) -> anyhow::Result<Url> {
        let url = match renderer.context().overrides.get(&OverrideKey::Url) {
            // Use the normal value
            None => renderer
                .render::<String>(&self.url)
                .await
                .context("Error rendering URL")?,
            // We need a URL! Omission triggers an error. Frontends should
            // probably prevent this
            Some(OverrideValue::Omit) => bail!("URL cannot be omitted"),
            Some(OverrideValue::Override(value)) => value.to_owned(),
        };
        url.parse::<Url>()
            .with_context(|| format!("Invalid URL: `{url}`"))
    }

    /// Render query key=value params
    async fn render_query(
        &self,
        renderer: &Renderer,
    ) -> anyhow::Result<Vec<(String, String)>> {
        let overrides = &renderer.context().overrides;
        // A param may have multiple values, so query_iter() disambiguates by
        // including an index param
        let futures = self.query_iter().map(|(param, i, procedure)| {
            with_override(
                overrides.get(&OverrideKey::Query(param.into(), i)),
                async move {
                    let value = renderer.render(procedure).await.context(
                        format!("Query parameter `{param}` (value `{i}`"),
                    )?;
                    Ok::<_, anyhow::Error>((param.to_owned(), value))
                },
                |value| Ok((param.to_owned(), value.to_owned())),
            )
        });
        // We have to filter out the omitted fields now
        let options = try_join_all(futures)
            .await
            .context("Error rendering query parameters")?;
        Ok(options.into_iter().flatten().collect())
    }

    /// Render all headers specified by the user. This will *not* include
    /// authentication and other implicit headers
    async fn render_headers(
        &self,
        renderer: &Renderer,
    ) -> anyhow::Result<HeaderMap> {
        // Render all headers concurrently
        render_all(
            renderer,
            self.headers.iter().map(|(k, v)| (k.as_str(), v)),
            OverrideKey::Header,
        )
        .await
        .context("Error rendering headers")?
        .into_iter()
        .map(|(header, value): (String, Bytes)| {
            let mut value: Vec<u8> = value.into();

            // Strip leading/trailing line breaks because they're going to
            // trigger a validation error and are probably a
            // mistake. We're trading explicitness for convenience
            // here.
            trim_bytes(&mut value, |c| c == b'\n' || c == b'\r');

            let header: HeaderName = header
                .clone()
                .try_into()
                .context(format!("Error encoding header name `{header}`"))?;
            let value: HeaderValue = value.try_into().context(format!(
                "Error encoding value for header `{header}`"
            ))?;
            Ok::<_, anyhow::Error>((header, value))
        })
        .collect()
    }

    /// Render authentication and return the same data structure, with resolved
    /// data. This can be passed to [reqwest::RequestBuilder]
    async fn render_authentication(
        &self,
        renderer: &Renderer,
    ) -> anyhow::Result<Option<Authentication<String>>> {
        let overrides = &renderer.context().overrides;
        match self.authentication.as_ref() {
            Some(Authentication::Basic { username, password }) => {
                let (username, password) = try_join!(
                    with_override(
                        overrides.get(&OverrideKey::AuthenticationUsername),
                        async {
                            renderer
                                .render(username)
                                .await
                                .context("Error rendering username")
                        },
                        |value| Ok(value.to_owned())
                    ),
                    with_override(
                        overrides.get(&OverrideKey::AuthenticationPassword),
                        async {
                            renderer
                                .render(password)
                                .await
                                .context("Error rendering password")
                        },
                        |value| Ok(value.to_owned())
                    )
                )?;
                let authentication = match (username, password) {
                    // If both fields have been omitted, exclude the header
                    (None, None) => None,
                    // If either field is omitted, replace it with an empty
                    // string which is effectively the same thing. This doesn't
                    // really make sense to do with a username, but might as
                    // well support it
                    (username, password) => Some(Authentication::Basic {
                        username: username.unwrap_or_default(),
                        password: password.unwrap_or_default(),
                    }),
                };
                Ok(authentication)
            }

            Some(Authentication::Bearer { token }) => {
                with_override(
                    overrides.get(&OverrideKey::AuthenticationToken),
                    async {
                        let token = renderer
                            .render(token)
                            .await
                            .context("Error rendering bearer token")?;
                        Ok(Authentication::Bearer { token })
                    },
                    |token| {
                        Ok(Authentication::Bearer {
                            token: token.to_owned(),
                        })
                    },
                )
                .await
            }
            None => Ok(None),
        }
    }

    /// Render request body
    async fn render_body(
        &self,
        renderer: &Renderer,
    ) -> anyhow::Result<Option<RenderedBody>> {
        let overrides = &renderer.context().overrides;
        let Some(body) = self.body.as_ref() else {
            return Ok(None);
        };

        match body {
            RecipeBody::Raw { data, .. } => {
                with_override(
                    overrides.get(&OverrideKey::Body),
                    async {
                        let body = renderer
                            .render::<Bytes>(data)
                            .await
                            .context("Error rendering body")?;
                        Ok(RenderedBody::Raw(body))
                    },
                    |body| Ok(RenderedBody::Raw(body.to_owned().into())),
                )
                .await
            }
            RecipeBody::Json { data } => {
                with_override(
                    overrides.get(&OverrideKey::Body),
                    async {
                        let value = renderer
                            .render::<Value>(data)
                            .await
                            .context("Error rendering JSON body")?;
                        // Convert from PetitScript to JSON. _Should_ be
                        // infallible
                        let json = serde_json::to_value(value)
                            .context("Error serializing JSON body")?;
                        Ok(RenderedBody::Json(json))
                    },
                    // Override value is a string; parse it as JSON. This
                    // allows us to pass it back to reqwest as a JSON body, so
                    // we can use the same downstream code path
                    |value| {
                        let json = serde_json::from_str(value)
                            .context("Error parsing body as JSON")?;
                        Ok(RenderedBody::Json(json))
                    },
                )
                .await
            }
            RecipeBody::FormUrlencoded { data } => {
                let rendered = render_all(
                    renderer,
                    data.iter().map(|(k, v)| (k.as_str(), v)),
                    OverrideKey::Form,
                )
                .await
                .context("Error rendering form fields")?;
                Ok(Some(RenderedBody::FormUrlencoded(rendered)))
            }
            RecipeBody::FormMultipart { data } => {
                let rendered = render_all(
                    renderer,
                    data.iter().map(|(k, v)| (k.as_str(), v)),
                    OverrideKey::Form,
                )
                .await
                .context("Error rendering form fields")?;
                Ok(Some(RenderedBody::FormMultipart(rendered)))
            }
        }
    }
}

impl Authentication<String> {
    fn apply(self, builder: RequestBuilder) -> RequestBuilder {
        match self {
            Authentication::Basic { username, password } => {
                builder.basic_auth(username, Some(password))
            }
            Authentication::Bearer { token } => builder.bearer_auth(token),
        }
    }
}

/// Body ready to be added to the request. Each variant corresponds to a method
/// by which we'll add it to the request. This means it is **not** 1:1 with
/// [RecipeBody]
#[derive(Debug)]
enum RenderedBody {
    Raw(Bytes),
    Json(serde_json::Value),
    /// Field:value mapping. Value is `String` because only string data can be
    /// URL-encoded
    FormUrlencoded(Vec<(String, String)>),
    /// Field:value mapping. Values can be arbitrary bytes
    FormMultipart(Vec<(String, Bytes)>),
}

impl RenderedBody {
    fn apply(self, builder: RequestBuilder) -> RequestBuilder {
        // Set body. The variant tells us _how_ to set it
        match self {
            RenderedBody::Raw(bytes) => builder.body(bytes),
            RenderedBody::Json(value) => builder.json(&value),
            RenderedBody::FormUrlencoded(fields) => builder.form(&fields),
            RenderedBody::FormMultipart(fields) => {
                let mut form = Form::new();
                for (field, value) in fields {
                    let part = Part::bytes(Vec::from(value));
                    form = form.part(field, part);
                }
                builder.multipart(form)
            }
        }
    }
}

impl From<HttpMethod> for reqwest::Method {
    fn from(method: HttpMethod) -> Self {
        match method {
            HttpMethod::Connect => reqwest::Method::CONNECT,
            HttpMethod::Delete => reqwest::Method::DELETE,
            HttpMethod::Get => reqwest::Method::GET,
            HttpMethod::Head => reqwest::Method::HEAD,
            HttpMethod::Options => reqwest::Method::OPTIONS,
            HttpMethod::Patch => reqwest::Method::PATCH,
            HttpMethod::Post => reqwest::Method::POST,
            HttpMethod::Put => reqwest::Method::PUT,
            HttpMethod::Trace => reqwest::Method::TRACE,
        }
    }
}

/// Render a sequence of (key, procedure) pairs. Each field can be overridden.
/// The procedures can be rendered to either strings or bytes, as needed.
async fn render_all<'a, V>(
    renderer: &Renderer,
    iter: impl Iterator<Item = (&'a str, &'a Procedure)>,
    override_key_fn: impl Fn(String) -> OverrideKey,
) -> anyhow::Result<Vec<(String, V)>>
where
    V: FromRendered,
{
    let overrides = &renderer.context().overrides;
    let futures = iter.map(|(key, procedure)| {
        with_override::<(String, V)>(
            overrides.get(&override_key_fn(key.into())),
            async move {
                let value = renderer
                    .render::<V>(procedure)
                    .await
                    .context(format!("Field `{key}`"))?;
                Ok::<_, anyhow::Error>((key.to_owned(), value))
            },
            |value| Ok((key.to_owned(), value.to_owned().into())),
        )
    });
    // We have to filter out the omitted fields now
    let options = try_join_all(futures).await?;
    Ok(options.into_iter().flatten().collect())
}

/// If an override value is present, use it. If the override says to omit the
/// field, return `Ok(None)`. Otherwise, call the render function and return
/// that value. The given `map` function is applied to the output string,
/// whether it was rendered or from the override.
async fn with_override<T>(
    override_value: Option<&OverrideValue>,
    render_future: impl Future<Output = anyhow::Result<T>>,
    map_override: impl FnOnce(&str) -> anyhow::Result<T>,
) -> anyhow::Result<Option<T>> {
    match override_value {
        None => {
            let value = render_future.await?;
            Ok(Some(value))
        }
        Some(OverrideValue::Omit) => Ok(None),
        Some(OverrideValue::Override(value)) => Ok(Some(map_override(value)?)),
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
