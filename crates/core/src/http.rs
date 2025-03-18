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
pub mod query;
#[cfg(test)]
mod tests;

pub use models::*;

use crate::{
    collection::{Authentication, Recipe, RecipeBody},
    db::CollectionDatabase,
    http::{content_type::ContentType, curl::CurlBuilder},
    template::{Template, TemplateContext},
    util::ResultTraced,
};
use anyhow::Context;
use bytes::Bytes;
use chrono::Utc;
use futures::{
    Future,
    future::{self, OptionFuture, try_join_all},
    try_join,
};
use mime::Mime;
use reqwest::{
    Client, RequestBuilder, Response, Url,
    header::{self, HeaderMap, HeaderName, HeaderValue},
    multipart::{Form, Part},
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use tracing::{info, info_span};

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
    persist: bool,
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
            persist: config.persist,
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
            recipe_id,
            options,
        } = &seed;
        let _ =
            info_span!("Build request", request_id = %id, ?recipe_id, ?options)
                .entered();

        struct Output<'a> {
            client: &'a Client,
            request: reqwest::Request,
            persist: bool,
        }
        let future = async {
            let recipe = template_context
                .collection
                .recipes
                .try_get_recipe(recipe_id)?;

            // Render everything up front so we can parallelize it
            let (url, query, headers, authentication, body) = try_join!(
                recipe.render_url(template_context),
                recipe.render_query(options, template_context),
                recipe.render_headers(options, template_context),
                recipe.render_authentication(options, template_context),
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
            Ok(Output {
                client,
                request,
                // Only persist if the global AND local options are enabled
                persist: self.persist && recipe.persist,
            })
        };
        let Output {
            client,
            request,
            persist,
        } = seed.run_future(future, template_context).await?;

        Ok(RequestTicket {
            record: RequestRecord::new(
                seed,
                template_context.selected_profile.clone(),
                &request,
                self.large_body_size,
            )
            .into(),
            client: client.clone(),
            request,
            persist,
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
            recipe_id,
            options,
        } = &seed;
        let _ =
            info_span!("Build request URL", request_id = %id, ?recipe_id, ?options)
                .entered();

        let future = async {
            let recipe = template_context
                .collection
                .recipes
                .try_get_recipe(recipe_id)?;

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
        let request = seed.run_future(future, template_context).await?;

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
            recipe_id,
            options,
        } = &seed;
        let _ =
            info_span!("Build request body", request_id = %id, ?recipe_id, ?options)
                .entered();

        let future = async {
            let recipe = template_context
                .collection
                .recipes
                .try_get_recipe(recipe_id)?;

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
                RenderedBody::FormUrlencoded(_)
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
        seed.run_future(future, template_context).await
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
        template_context: &TemplateContext,
    ) -> Result<String, RequestBuildError> {
        let RequestSeed {
            id,
            recipe_id,
            options,
        } = &seed;
        let _ =
            info_span!("Build request cURL", request_id = %id, ?recipe_id, ?options)
                .entered();

        let future = async {
            let recipe = template_context
                .collection
                .recipes
                .try_get_recipe(recipe_id)?;

            // Render everything up front so we can parallelize it
            let (url, query, headers, authentication, body) = try_join!(
                recipe.render_url(template_context),
                recipe.render_query(options, template_context),
                recipe.render_headers(options, template_context),
                recipe.render_authentication(options, template_context),
                recipe.render_body(options, template_context),
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
        seed.run_future(future, template_context).await
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

#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct HttpEngineConfig {
    /// TLS cert errors on these hostnames are ignored. Be careful!
    pub ignore_certificate_hosts: Vec<String>,
    /// Request/response bodies over this size are treated differently, for
    /// performance reasons
    pub large_body_size: usize,
    /// Enable/disable persistence for _all_ requests? The CLI should override
    /// this based on the absence/presence of the `--persist` flag
    pub persist: bool,
}

impl HttpEngineConfig {
    /// Is the given size (e.g. request or response body size) larger than the
    /// configured "large" body size? Large bodies are treated differently, for
    /// performance reasons.
    pub fn is_large(&self, size: usize) -> bool {
        size > self.large_body_size
    }
}

impl Default for HttpEngineConfig {
    fn default() -> Self {
        Self {
            ignore_certificate_hosts: Default::default(),
            large_body_size: 1000 * 1000, // 1MB
            persist: true,
        }
    }
}

impl RequestSeed {
    /// Run the given future and convert any error into [RequestBuildError]
    async fn run_future<T>(
        &self,
        future: impl Future<Output = anyhow::Result<T>>,
        template_context: &TemplateContext,
    ) -> Result<T, RequestBuildError> {
        let start_time = Utc::now();
        future.await.traced().map_err(|error| RequestBuildError {
            profile_id: template_context.selected_profile.clone(),
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

                if self.persist {
                    // Error here should *not* kill the request
                    let _ = database.insert_exchange(&exchange);
                }
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
        let iter = self.query.iter().enumerate().filter_map(|(i, (k, v))| {
            // Look up and apply override. We do this by index because the
            // keys aren't necessarily unique
            let template = options.query_parameters.get(i, v)?;

            Some(async move {
                Ok::<_, anyhow::Error>((
                    k.clone(),
                    template.render_string(template_context).await.context(
                        format!("Error rendering query parameter `{k}`"),
                    )?,
                ))
            })
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
            self.body.as_ref().and_then(|body| body.explicit_mime())
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
        let iter = self.headers.iter().enumerate().filter_map(
            move |(i, (header, value_template))| {
                // Look up and apply override. We do this by index because the
                // keys aren't necessarily unique
                let template = options.headers.get(i, value_template)?;

                Some(async move {
                    self.render_header(template_context, header, template).await
                })
            },
        );

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
        options: &BuildOptions,
        template_context: &TemplateContext,
    ) -> anyhow::Result<Option<Authentication<String>>> {
        let authentication = options
            .authentication
            .as_ref()
            .or(self.authentication.as_ref());
        match authentication {
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
        let Some(body) = options.body.as_ref().or(self.body.as_ref()) else {
            return Ok(None);
        };

        let rendered = match body {
            RecipeBody::Raw { body, .. } => RenderedBody::Raw(
                body.render(template_context)
                    .await
                    .context("Error rendering body")?
                    .into(),
            ),
            RecipeBody::FormUrlencoded(fields) => {
                let iter = fields.iter().enumerate().filter_map(
                    |(i, (field, value_template))| {
                        let template =
                            options.form_fields.get(i, value_template)?;
                        Some(async move {
                            let value = template
                                .render_string(template_context)
                                .await
                                .context(format!(
                                    "Error rendering form field `{field}`"
                                ))?;
                            Ok::<_, anyhow::Error>((field.clone(), value))
                        })
                    },
                );
                let rendered = try_join_all(iter).await?;
                RenderedBody::FormUrlencoded(rendered)
            }
            RecipeBody::FormMultipart(fields) => {
                let iter = fields.iter().enumerate().filter_map(
                    |(i, (field, value_template))| {
                        let template =
                            options.form_fields.get(i, value_template)?;
                        Some(async move {
                            let value = template
                                .render(template_context)
                                .await
                                .context(format!(
                                    "Error rendering form field `{field}`"
                                ))?;
                            Ok::<_, anyhow::Error>((field.clone(), value))
                        })
                    },
                );
                let rendered = try_join_all(iter).await?;
                RenderedBody::FormMultipart(rendered)
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

impl RecipeBody {
    /// Get the value that we should set for the `Content-Type` header,
    /// according to the body. This will only return `Some` for JSON, as the
    /// form content types will have this header set automatically by reqwest
    /// via the builder methods we use.
    fn explicit_mime(&self) -> Option<Mime> {
        match self {
            RecipeBody::Raw { content_type, .. } => {
                content_type.as_ref().map(ContentType::to_mime)
            }
            // Do *not* set anything for these, because reqwest will do that
            // automatically and we don't want to interfere
            RecipeBody::FormUrlencoded(_) | RecipeBody::FormMultipart(_) => {
                None
            }
        }
    }
}

/// Body ready to be added to the request. Each variant corresponds to a method
/// by which we'll add it to the request. This means it is **not** 1:1 with
/// [RecipeBody]
enum RenderedBody {
    Raw(Bytes),
    /// Field:value mapping. Value is `String` because only string data can be
    /// URL-encoded
    FormUrlencoded(Vec<(String, String)>),
    /// Field:value mapping. Values can be arbitrary bytes
    FormMultipart(Vec<(String, Vec<u8>)>),
}

impl RenderedBody {
    fn apply(self, builder: RequestBuilder) -> RequestBuilder {
        // Set body. The variant tells us _how_ to set it
        match self {
            RenderedBody::Raw(bytes) => builder.body(bytes),
            RenderedBody::FormUrlencoded(fields) => builder.form(&fields),
            RenderedBody::FormMultipart(fields) => {
                let mut form = Form::new();
                for (field, value) in fields {
                    let part = Part::bytes(value);
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
