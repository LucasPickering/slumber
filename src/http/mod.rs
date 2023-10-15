//! HTTP-specific logic and models. [HttpEngine] is the main entrypoint for all
//! operations.

mod parse;
mod record;
mod repository;

pub use parse::*;
pub use record::*;
pub use repository::*;

use crate::{
    config::RequestRecipe, template::TemplateContext, util::ResultExt,
};
use anyhow::Context;
use chrono::Utc;
use futures::future;
use indexmap::IndexMap;
use reqwest::{
    header::{HeaderName, HeaderValue},
    Client,
};
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
#[derive(Clone, Debug)]
pub struct HttpEngine {
    client: Client,
    repository: Repository,
}

impl HttpEngine {
    /// Build a new HTTP engine, which can be used for the entire program life
    pub fn new(repository: Repository) -> Self {
        Self {
            client: Client::builder()
                .user_agent(USER_AGENT)
                .build()
                // This should be infallible
                .expect("Error building reqwest client"),
            repository,
        }
    }

    /// Instantiate a request from a recipe, using values from the given
    /// context to render templated strings. Errors if request construction
    /// fails because of invalid user input somewhere.
    pub async fn build_request(
        recipe: &RequestRecipe,
        template_values: &TemplateContext,
    ) -> anyhow::Result<Request> {
        debug!(recipe_id = %recipe.id, "Building request from recipe");
        let method = recipe
            .method
            .render(template_values, "method")
            .await?
            .parse()?;
        let url = recipe.url.render(template_values, "URL").await?;

        // Build header map
        let headers = future::try_join_all(recipe.headers.iter().map(
            |(header, value_template)| async move {
                // String -> header conversions are fallible, if headers
                // are invalid
                Ok::<_, anyhow::Error>((
                    HeaderName::try_from(header).with_context(|| {
                        format!("Error parsing header name {header:?}")
                    })?,
                    HeaderValue::try_from(
                        value_template
                            .render(
                                template_values,
                                &format!("header {header}"),
                            )
                            .await?,
                    )?,
                ))
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
                    v.render(template_values, &format!("Query parameter {k}"))
                        .await?,
                ))
            }),
        )
        .await?
        .into_iter()
        .collect();
        // Render the body
        let body = match &recipe.body {
            Some(body) => Some(
                body.render(template_values, "body").await.context("Body")?,
            ),
            None => None,
        };

        let request = Request {
            recipe_id: recipe.id.clone(),
            method,
            url,
            query,
            body,
            headers,
        };
        info!(
            recipe_id = %recipe.id,
            "Built request from recipe",
        );
        Ok(request)
    }

    /// Launch an HTTP request. Upon completion, it will automatically be
    /// registered in the repository for posterity.
    ///
    /// This consumes the HTTP engine so that the future can outlive the scope
    /// that created the future. This allows the future to be created outside
    /// the task that will resolve it.
    ///
    /// Returns a full HTTP record, which includes the originating request, the
    /// response, and the start/end timestamps.
    pub async fn send(self, request: Request) -> anyhow::Result<RequestRecord> {
        let id: RequestId = RequestId::new();
        let start_time = Utc::now();

        let span = info_span!("HTTP request", request_id = %id);
        let reqwest_request = self.convert_request(&request);
        span.in_scope(|| async move {
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
            let end_time = Utc::now();

            info!(status = response.status.as_u16(), "Response");
            let record = RequestRecord {
                id,
                request,
                response,
                start_time,
                end_time,
            };
            // Error here should *not* kill the request
            let _ = self.repository.insert(&record).await;
            Ok(record)
        })
        .await
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
            // TODO this case is possible with an invalid URL
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
