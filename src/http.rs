//! This whole module is basically a wrapper around reqwest to make it more
//! ergnomic for our needs

use crate::{config::RequestRecipe, template::TemplateValues};
use anyhow::Context;
use reqwest::{
    header::{HeaderMap, HeaderName},
    Client, Method, StatusCode,
};
use std::{collections::HashMap, ops::Deref};
use tracing::trace;

static USER_AGENT: &str =
    concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"),);

/// Utility for handling all HTTP operations. The main purpose of this is to
/// de-asyncify HTTP so it can be called in the main TUI thread. All heavy
/// lifting will be pushed to background tasks.
///
/// This is safe and cheap to clone because reqwest's `Client` type uses `Arc`
/// internally.
/// https://docs.rs/reqwest/0.11.20/reqwest/struct.Client.html
#[derive(Clone, Debug, Default)]
pub struct HttpEngine {
    client: Client,
}

/// A single instance of an HTTP request. Simpler alternative to
/// [reqwest::Request] that suits our needs better.
#[derive(Clone, Debug)]
pub struct Request {
    pub method: Method,
    pub url: String,
    pub headers: HeaderMap,
    pub query: HashMap<String, String>,
    /// Text body content. At some point we'll support other formats (binary,
    /// streaming from file, etc.)
    pub body: Option<String>,
}

/// A resolved HTTP response, with all content loaded and ready to be displayed
/// to the user. A simpler alternative to [reqwest::Response], because there's
/// no way to access all resolved data on that type at once. Resolving the
/// response body requires moving the response.
#[derive(Clone, Debug)]
pub struct Response {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub content: String,
}

impl HttpEngine {
    pub fn new() -> Self {
        Self {
            client: Client::builder()
                .user_agent(USER_AGENT)
                .build()
                .expect("Error building reqwest client"),
        }
    }

    /// Instantiate a request from a recipe, using values from the given
    /// environment to render templated strings. Errors if request construction
    /// fails because of invalid user input somewhere.
    pub fn build_request(
        &self,
        recipe: &RequestRecipe,
        template_values: &TemplateValues,
    ) -> anyhow::Result<Request> {
        // TODO add more tracing
        let method = recipe
            .method
            .render(template_values)
            .context("Method")?
            .parse()?;
        let url = recipe.url.render(template_values).context("URL")?;

        // Build header map
        let mut headers = HeaderMap::new();
        for (key, value_template) in &recipe.headers {
            trace!(key = key, value = value_template.deref(), "Adding header");
            headers.append(
                key.parse::<HeaderName>()
                    // TODO do we need this context? is the base error good
                    // enough?
                    .context("Error parsing header name")?,
                value_template
                    .render(template_values)
                    .with_context(|| format!("Header {key:?}"))?
                    // I'm not sure when this parse would fail, it seems like
                    // the value can be any bytes
                    // https://docs.rs/reqwest/0.11.20/reqwest/header/struct.HeaderValue.html
                    .parse()
                    .context("Error parsing header value")?,
            );
        }

        // Add query parameters
        let query = recipe
            .query
            .iter()
            .map(|(k, v)| {
                Ok((
                    k.clone(),
                    v.render(template_values)
                        .with_context(|| format!("Query parameter {k:?}"))?,
                ))
            })
            .collect::<anyhow::Result<HashMap<_, _>>>()?;
        let body = recipe
            .body
            .as_ref()
            .map(|body| body.render(template_values).context("Body"))
            .transpose()?;
        Ok(Request {
            method,
            url,
            query,
            body,
            headers,
        })
    }

    /// Launch a request in a spawned task. The response will be stored with
    /// the request
    pub async fn send_request(
        &self,
        request: Request,
    ) -> reqwest::Result<Response> {
        // Convert to reqwest's request format
        let mut request_builder = self
            .client
            .request(request.method, request.url)
            .headers(request.headers)
            .query(&request.query);
        if let Some(body) = request.body {
            request_builder = request_builder.body(body);
        }
        // Failure here is a bug
        let reqwest_request = request_builder
            .build()
            .expect("Error building HTTP request");

        let reqwest_response = self.client.execute(reqwest_request).await?;

        // Copy response data out first, because we need to move the
        // response to resolve content (not sure why...)
        let status = reqwest_response.status();
        let headers = reqwest_response.headers().clone();

        // Pre-resolve the content, so we get all the async work done
        let content = reqwest_response.text().await?;

        Ok(Response {
            status,
            headers,
            content,
        })
    }
}
