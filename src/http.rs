//! This whole module is basically a wrapper around reqwest to make it more
//! ergnomic for our needs. This doesn't manage any state, it's a purely
//! functional adapter for making HTTP requests.

use crate::{
    config::{RequestRecipe, RequestRecipeId},
    template::TemplateContext,
    util::ResultExt,
};
use anyhow::Context;
use derive_more::{Deref, Display, From};
use futures::future;
use indexmap::IndexMap;
use reqwest::{
    header::{HeaderMap, HeaderName, HeaderValue},
    Client, Method, StatusCode,
};
use serde::{Deserialize, Serialize};
use std::future::Future;
use tracing::{debug, info, info_span};
use uuid::Uuid;

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

/// Unique ID for a single launched request
#[derive(
    Copy,
    Clone,
    Debug,
    Deref,
    Display,
    Eq,
    From,
    Hash,
    PartialEq,
    Serialize,
    Deserialize,
)]
pub struct RequestId(Uuid);

/// A single instance of an HTTP request. Simpler alternative to
/// [reqwest::Request] that suits our needs better. This intentionally does
/// *not* implement `Clone`, because each request is unique.
#[derive(Debug, Serialize, Deserialize)]
pub struct Request {
    /// ID to uniquely refer to this request. Useful for historical records.
    pub id: RequestId,
    /// The recipe used to generate this request (for historical context)
    pub recipe_id: RequestRecipeId,

    #[serde(with = "serde_method")]
    pub method: Method,
    pub url: String,
    #[serde(with = "serde_header_map")]
    pub headers: HeaderMap,
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
    #[serde(with = "serde_header_map")]
    pub headers: HeaderMap,
    pub body: String,
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

    /// Launch an HTTP request. The caller is responsible for registering the
    /// given request, and returned response/error, in the repository.
    ///
    /// This consumes the HTTP engine so that the future can outlive the scope
    /// that created the future. This allows the future to be created outside
    /// the task that will resolve it.
    ///
    /// This returns `impl Future` instead of being `async` so we can detach the
    /// lifetime of the request from the future.
    pub fn send<'a>(
        self,
        request: &Request,
    ) -> impl Future<Output = anyhow::Result<Response>> + 'a {
        // Convert the request *outside* the future, so we can drop the
        // reference to the record
        let reqwest_request = self.convert_request(request);
        let span = info_span!("HTTP request", request_id = %request.id);

        async move {
            span.in_scope(|| async {
                // Any error inside this block should be persisted in the repo

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
            .await
        }
    }

    /// Convert from our request type to reqwest's. The input request should
    /// already be validated by virtue of its type structure, so this conversion
    /// is generally infallible. There is potential for an error though, which
    /// will trigger a panic. Hopefully that never happens!
    ///
    /// This will pretty much clone all the data out of the request, which sucks
    /// but there's no alternative. Reqwest wants to own it all, but we also
    /// need to retain ownership for the UI.
    fn convert_request(&self, request: &Request) -> reqwest::Request {
        // Convert to reqwest's request format
        let mut request_builder = self
            .client
            .request(request.method.clone(), &request.url)
            .query(&request.query)
            .headers(request.headers.clone());

        // Add body
        if let Some(body) = &request.body {
            request_builder = request_builder.body(body.clone());
        }

        // An error here indicates a bug. Technically we should just show the
        // error to the user, but panicking saves us from a lot of grungy logic.
        request_builder
            .build()
            .expect("Error building HTTP request (this is a bug!)")
    }

    /// Convert reqwest's response type into ours. This is async because the
    /// response content is not necessarily loaded when we first get the
    /// response. Only fallible if the response content fails to load.
    async fn convert_response(
        &self,
        response: reqwest::Response,
    ) -> anyhow::Result<Response> {
        // Copy response metadata out first, because we need to move the
        // response to resolve content (not sure why...)
        let status = response.status();
        let headers = response.headers().clone();

        // Pre-resolve the content, so we get all the async work done
        let body = response.text().await?;

        Ok(Response {
            status,
            headers,
            body,
        })
    }
}

impl Request {
    /// Instantiate a request from a recipe, using values from the given
    /// environment to render templated strings. Errors if request construction
    /// fails because of invalid user input somewhere.
    pub async fn build(
        recipe: &RequestRecipe,
        template_values: &TemplateContext,
    ) -> anyhow::Result<Self> {
        debug!(recipe_id = %recipe.id, "Building request from recipe");
        let method = recipe
            .method
            .render(template_values)
            .await
            .context("Method")?
            .parse()
            .context("Method")?;
        let url = recipe.url.render(template_values).await.context("URL")?;

        // Build header map
        let headers = future::try_join_all(recipe.headers.iter().map(
            |(header, value_template)| async move {
                let result: anyhow::Result<_> = try {
                    // String -> header conversions are fallible, if headers
                    // are invalid
                    (
                        HeaderName::try_from(header)?,
                        HeaderValue::try_from(
                            value_template.render(template_values).await?,
                        )?,
                    )
                };
                result.with_context(|| format!("Header {header:?}"))
            },
        ))
        .await?
        .into_iter()
        .collect();

        // Add query parameters
        let query: IndexMap<String, String> = future::try_join_all(
            recipe.query.iter().map(|(k, v)| async move {
                Ok::<_, anyhow::Error>((
                    k.clone(),
                    v.render(template_values)
                        .await
                        .with_context(|| format!("Query parameter {k:?}"))?,
                ))
            }),
        )
        .await?
        .into_iter()
        .collect();
        // Render the body
        let body = match &recipe.body {
            Some(body) => {
                Some(body.render(template_values).await.context("Body")?)
            }
            None => None,
        };

        let request = Self {
            id: RequestId(Uuid::new_v4()),
            recipe_id: recipe.id.clone(),
            method,
            url,
            query,
            body,
            headers,
        };
        info!(
            request_id = %request.id,
            recipe_id = %recipe.id,
            "Built request from recipe",
        );
        Ok(request)
    }
}

/// Serialization/deserialization for [reqwest::Method]
mod serde_method {
    use super::*;
    use serde::{de, Deserializer, Serializer};

    pub fn serialize<S>(
        method: &Method,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(method.as_str())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Method, D::Error>
    where
        D: Deserializer<'de>,
    {
        <&str>::deserialize(deserializer)?
            .parse()
            .map_err(de::Error::custom)
    }
}

/// Serialization/deserialization for [reqwest::HeaderMap]
mod serde_header_map {
    use super::*;
    use reqwest::header::{HeaderName, HeaderValue};
    use serde::{de, Deserializer, Serializer};

    pub fn serialize<S>(
        headers: &HeaderMap,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // HeaderValue -> str is fallible, so we'll serialize as bytes instead
        <IndexMap<&str, &[u8]>>::serialize(
            &headers
                .into_iter()
                .map(|(k, v)| (k.as_str(), v.as_bytes()))
                .collect(),
            serializer,
        )
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<HeaderMap, D::Error>
    where
        D: Deserializer<'de>,
    {
        <IndexMap<String, Vec<u8>>>::deserialize(deserializer)?
            .into_iter()
            .map::<Result<(HeaderName, HeaderValue), _>, _>(|(k, v)| {
                // Fallibly map each key and value to header types
                Ok((
                    k.try_into().map_err(de::Error::custom)?,
                    v.try_into().map_err(de::Error::custom)?,
                ))
            })
            .try_collect()
    }
}

/// Serialization/deserialization for [reqwest::StatusCode]
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
