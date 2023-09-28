//! This whole module is basically a wrapper around reqwest to make it more
//! ergnomic for our needs

use crate::{
    config::{RequestRecipe, RequestRecipeId},
    history::RequestHistory,
    template::TemplateContext,
    util::ResultExt,
};
use anyhow::Context;
use indexmap::IndexMap;
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, info_span};

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
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Request {
    /// The recipe used to generate this request (for historical context)
    pub recipe_id: RequestRecipeId,

    pub method: String,
    pub url: String,
    pub headers: IndexMap<String, String>,
    pub query: IndexMap<String, String>,
    /// Text body content. At some point we'll support other formats (binary,
    /// streaming from file, etc.)
    pub body: Option<String>,
}

/// A resolved HTTP response, with all content loaded and ready to be displayed
/// to the user. A simpler alternative to [reqwest::Response], because there's
/// no way to access all resolved data on that type at once. Resolving the
/// response body requires moving the response.
#[derive(Debug, Serialize, Deserialize)]
pub struct Response {
    #[serde(with = "serde_status_code")]
    pub status: StatusCode,
    pub headers: IndexMap<String, String>,
    pub content: String,
}

impl HttpEngine {
    pub fn new() -> Self {
        Self {
            client: Client::builder()
                .user_agent(USER_AGENT)
                .build()
                // This should be infallible
                .expect("Error building reqwest client"),
        }
    }

    /// Instantiate a request from a recipe, using values from the given
    /// environment to render templated strings. Errors if request construction
    /// fails because of invalid user input somewhere.
    ///
    /// The returned request is *not necessarily* a valid HTTP request.
    pub fn build_request(
        &self,
        recipe: &RequestRecipe,
        template_values: &TemplateContext,
    ) -> anyhow::Result<Request> {
        debug!(?recipe, "Building request from recipe");
        let method = recipe.method.render(template_values).context("Method")?;
        let url = recipe.url.render(template_values).context("URL")?;

        // Build header map
        let headers = recipe
            .headers
            .iter()
            .map(|(header, value_template)| {
                Ok((
                    header.clone(),
                    value_template
                        .render(template_values)
                        .with_context(|| format!("Header {header:?}"))?,
                ))
            })
            .collect::<anyhow::Result<_>>()?;

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
            .collect::<anyhow::Result<IndexMap<_, _>>>()?;
        let body = recipe
            .body
            .as_ref()
            .map(|body| body.render(template_values).context("Body"))
            .transpose()?;
        Ok(Request {
            recipe_id: recipe.id.clone(),
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
    ) -> anyhow::Result<Response> {
        // Open a new connection to history. This function may be called in an
        // async task so it can't be passed a connection
        let history = RequestHistory::load_fast()?;
        // TODO frontload this so the error happens during build
        // (should make the clone unnecessary too?)
        let reqwest_request = self
            .convert_request(request.clone())
            .context("Error building HTTP request")?;

        // Don't add request to history until we know we can launch it
        let request_id = history.add_request(&request)?;

        let span = info_span!("HTTP request", %request_id);
        let response_result = span
            .in_scope(|| async {
                info!(request_url = %reqwest_request.url());
                // Any error inside this block should be stored in history

                let reqwest_response = self
                    .client
                    .execute(reqwest_request)
                    .await
                    .map_err(anyhow::Error::from)
                    .traced()?;
                let response = self
                    .convert_response(reqwest_response)
                    .await
                    .context("Error loading response")
                    .traced()?;

                info!(status = response.status.as_u16(), "Response");
                Ok(response)
            })
            .await;

        // Store result (success OR failure) in history
        history.add_response(request_id, &response_result)?;
        response_result
    }

    /// Convert from our request type to reqwest's. We don't do any content
    /// validation on the input data, so this is where things like HTTP method,
    /// header names, etc. will be validated. It'd be nice if this was a TryFrom
    /// impl, but it requires context from the engine.
    fn convert_request(
        &self,
        request: Request,
    ) -> anyhow::Result<reqwest::Request> {
        // Convert to reqwest's request format
        let mut request_builder = self
            .client
            .request(request.method.parse()?, request.url)
            .query(&request.query);

        // Add headers
        // TODO support non-utf8 header values
        for (header, value) in request.headers {
            request_builder = request_builder.header(header, value);
        }

        // Add body
        if let Some(body) = request.body {
            request_builder = request_builder.body(body);
        }

        Ok(request_builder.build()?)
    }

    /// Convert reqwest's response type into ours. This is async because the
    /// response content is not necessarily loaded when we first get the
    /// response.
    async fn convert_response(
        &self,
        response: reqwest::Response,
    ) -> anyhow::Result<Response> {
        // Copy response metadata out first, because we need to move the
        // response to resolve content (not sure why...)
        let status = response.status();

        let headers = response
            .headers()
            .iter()
            .map(|(header, value)| {
                (
                    header.as_str().to_owned(),
                    // TODO support non-utf8 header values
                    value.to_str().unwrap().to_owned(),
                )
            })
            .collect();

        // Pre-resolve the content, so we get all the async work done
        let content = response.text().await?;

        Ok(Response {
            status,
            headers,
            content,
        })
    }
}

mod serde_status_code {
    use super::*;
    use serde::{de, Deserializer, Serializer};

    pub fn serialize<S>(
        status_code: &StatusCode,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u16(status_code.as_u16())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<StatusCode, D::Error>
    where
        D: Deserializer<'de>,
    {
        StatusCode::from_u16(u16::deserialize(deserializer)?)
            .map_err(de::Error::custom)
    }
}
