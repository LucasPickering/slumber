//! This whole module is basically a wrapper around reqwest to make it more
//! ergnomic for our needs

use crate::{config::RequestRecipe, template::TemplateValues};
use anyhow::Context;
use reqwest::{
    header::{HeaderMap, HeaderName},
    Client, Method, StatusCode,
};
use std::{ops::Deref, sync::Arc};
use tokio::{sync::RwLock, task::JoinHandle};
use tracing::trace;

static USER_AGENT: &str =
    concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"),);

/// Utility for handling all HTTP operations. The main purpose of this is to
/// de-asyncify HTTP so it can be called in the main TUI thread. All heavy
/// lifting will be pushed to background tasks.
#[derive(Clone, Debug, Default)]
pub struct HttpEngine {
    client: Client,
}

/// A single instance of an HTTP request, with an optional response. Most of
/// this is sync because it should be built on the main thread, but the request
/// gets sent async so the response has to be populated async
#[derive(Debug)]
pub struct Request {
    pub method: Method,
    pub url: String,
    pub headers: HeaderMap,
    /// Text body content. At some point we'll support other formats (binary,
    /// streaming from file, etc.)
    pub body: Option<String>,
    /// Resolved response, or an error. Since this gets populated
    /// asynchronously, we need to store it behind a lock
    pub response: Arc<RwLock<ResponseState>>,
}

/// State of a response, corresponding to a single request
#[derive(Debug, Default)]
pub enum ResponseState {
    /// Request hasn't been sent yet
    #[default]
    None,
    /// Response is in flight
    Loading,
    /// A resolved HTTP response, with all content loaded and ready to be
    /// displayed in the UI. This does *not necessarily* have a 2xx/3xx status
    /// code, any received response is stored here.
    Complete {
        status: StatusCode,
        headers: HeaderMap,
        content: String,
    },
    /// Error occurred sending the request or receiving the response
    Error(reqwest::Error),
}

/// A resolved HTTP response, with all content loaded and ready to be displayed
/// in the UI. A simpler alternative to [reqwest::Response], because there's
/// no way to access all resolved data on that type at once. Resolving the
/// response body requires moving the response.
#[derive(Debug)]
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
    /// environment to render templated strings
    pub fn build_request(
        &self,
        recipe: &RequestRecipe,
        template_values: &TemplateValues,
    ) -> anyhow::Result<Request> {
        // TODO add more tracing
        let method = recipe.method.render(template_values)?.parse()?;
        let url = recipe.url.render(template_values)?;

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
                    .with_context(|| {
                        format!("Error rendering value for header {key}")
                    })?
                    // I'm not sure when this parse would fail, it seems like
                    // the value can be any bytes
                    // https://docs.rs/reqwest/0.11.20/reqwest/header/struct.HeaderValue.html
                    .parse()
                    .context("Error parsing header value")?,
            );
        }

        let body = recipe
            .body
            .as_ref()
            .map(|body| body.render(template_values))
            .transpose()?;
        Ok(Request {
            method,
            url,
            body,
            headers,
            response: Arc::new(RwLock::new(ResponseState::None)),
        })
    }

    /// Launch a request in a spawned task. The response will be stored with
    /// the request
    pub fn send_request(&self, request: &Request) -> JoinHandle<()> {
        // Convert to reqwest's request format
        let mut request_builder = self
            .client
            .request(request.method.clone(), request.url.clone())
            .headers(request.headers.clone());
        if let Some(body) = &request.body {
            request_builder = request_builder.body(body.clone());
        }
        // Failure here is a bug
        let reqwest_request = request_builder
            .build()
            .expect("Error building HTTP request");

        // Launch the request in a task
        let response_box = Arc::clone(&request.response);
        // Client is safe to clone, it uses Arc internally
        // https://docs.rs/reqwest/0.11.20/reqwest/struct.Client.html
        let client = self.client.clone();
        tokio::spawn(async move {
            // Immediately mark the response as loading
            *response_box.write().await = ResponseState::Loading;

            // Execute the request and get all response metadata/content.
            // This double-result thing is needed because we can't impl
            // FromResidual on stable.
            let result: Result<ResponseState, ResponseState> = async {
                let reqwest_response = client.execute(reqwest_request).await?;

                // Copy response data out first, because we need to move the
                // response to resolve content (not sure why...)
                let status = reqwest_response.status();
                let headers = reqwest_response.headers().clone();

                // Pre-resolve the content, so we get all the async work done
                let content = reqwest_response.text().await?;

                Ok(ResponseState::Complete {
                    status,
                    headers,
                    content,
                })
            }
            .await;

            // Store the result with the request
            *response_box.write().await = result.into();
        })
    }
}

// These two impls can be replaced with a single FromResidiaul impl after
// https://github.com/rust-lang/rust/issues/84277 (or if we switch to nightly)
impl From<reqwest::Error> for ResponseState {
    fn from(error: reqwest::Error) -> Self {
        Self::Error(error)
    }
}

impl From<Result<Self, Self>> for ResponseState {
    fn from(value: Result<Self, Self>) -> Self {
        match value {
            Ok(value) => value,
            Err(value) => value,
        }
    }
}
