//! HTTP-specific logic and models. [HttpEngine] is the main entrypoint for all
//! operations. This is the life cycle of a request:
//!
//! +--------+
//! | Recipe |
//! +--------+
//!      |
//!     new
//!      |
//!      v
//! +----------------+          +-------------------+
//! | RequestBuilder | -error-> | RequestBuildError |
//! +----------------+          +-------------------+
//!      |
//!    build
//!      |
//!      v
//! +---------+
//! | Request |
//! +---------+
//!      |
//!    send
//!      |
//!      v
//! +----------+          +--------------+
//! | <future> | -error-> | RequestError |
//! +----------+          +--------------+
//!      |
//!   success
//!      |
//!      v
//! +---------------+
//! | RequestRecord |
//! +---------------+

mod parse;
mod record;
mod repository;

pub use parse::*;
pub use record::*;
pub use repository::*;

use crate::{
    collection::RequestRecipe, template::TemplateContext, util::ResultExt,
};
use anyhow::Context;
use chrono::Utc;
use futures::future;
use indexmap::IndexMap;
use reqwest::{
    header::{HeaderMap, HeaderName, HeaderValue},
    Client,
};
use tokio::try_join;
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

    /// Launch an HTTP request. Upon completion, it will automatically be
    /// registered in the repository for posterity.
    ///
    /// This consumes the HTTP engine so that the future can outlive the scope
    /// that created the future. This allows the future to be created outside
    /// the task that will resolve it.
    ///
    /// Returns a full HTTP record, which includes the originating request, the
    /// response, and the start/end timestamps. We can't report a reliable start
    /// time until after the future is resolved, because the request isn't
    /// launched until the consumer starts awaiting the future. For in-flight
    /// time tracking, track your own start time immediately before/after
    /// sending the request.
    pub async fn send(
        self,
        request: Request,
    ) -> Result<RequestRecord, RequestError> {
        let id = request.id;

        let span = info_span!("HTTP request", request_id = %id);
        span.in_scope(|| async move {
            // This start time will be accurate because the request doesn't
            // launch until this whole future is awaited

            // Technically the elapsed time will include the conversion time,
            // but that should be extremely minimal compared to network IO
            let start_time = Utc::now();
            let result: reqwest::Result<Response> = try {
                // Convert to reqwest format as part of the execution. This
                // means certain builder errors will show up as "request" errors
                // which is janky, but reqwest already doesn't report some
                // builder erorrs until you execute the request, and this is
                // much easier than frontloading the conversion during the build
                // process.
                let reqwest_request = self.convert_request(&request)?;
                let reqwest_response =
                    self.client.execute(reqwest_request).await?;
                // Load the full response and convert it to our format
                self.convert_response(reqwest_response).await?
            };
            let end_time = Utc::now();

            // Attach metadata to the error and yeet it
            match result {
                // Can't use map_err because we need to conditionally move
                // the request
                Ok(response) => {
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
                }
                Err(error) => Err(RequestError {
                    request,
                    start_time,
                    end_time,
                    error,
                })
                .traced(),
            }
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
    fn convert_request(
        &self,
        request: &Request,
    ) -> reqwest::Result<reqwest::Request> {
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

        request_builder.build()
    }

    /// Convert reqwest's response type into ours. This is async because the
    /// response content is not necessarily loaded when we first get the
    /// response. Only fallible if the response content fails to load.
    async fn convert_response(
        &self,
        response: reqwest::Response,
    ) -> reqwest::Result<Response> {
        // Copy response metadata out first, because we need to move the
        // response to resolve content (not sure why...)
        let status = response.status();
        let headers = response.headers().clone();

        // Pre-resolve the content, so we get all the async work done
        let body = response.text().await?.into();

        Ok(Response {
            status,
            headers,
            body,
        })
    }
}

/// The foundation of a request. This builder captures *how* the request will
/// be built, but it hasn't actually been built yet.
pub struct RequestBuilder {
    // Don't store start_time here because we don't need to track build time,
    // only in-flight time
    id: RequestId,
    // We need this during the build
    recipe: RequestRecipe,
    template_context: TemplateContext,
}

impl RequestBuilder {
    /// Instantiate new request builder for the given recipe. Use [Self::build]
    /// to build it.
    ///
    /// This needs an owned recipe and context so they can be moved into a
    /// subtask for the build.
    pub fn new(
        recipe: RequestRecipe,
        template_context: TemplateContext,
    ) -> RequestBuilder {
        debug!(recipe_id = %recipe.id, "Building request from recipe");
        let request_id = RequestId::new();

        Self {
            id: request_id,
            recipe,
            template_context,
        }
    }

    /// The unique ID generated for this request, which can be used to track it
    /// throughout its life cycle
    pub fn id(&self) -> RequestId {
        self.id
    }

    /// Build the request. This is async because templated values may require IO
    /// or other async actions.
    pub async fn build(self) -> Result<Request, RequestBuildError> {
        let id = self.id;
        self.build_helper()
            .await
            .traced()
            .map_err(|error| RequestBuildError { id, error })
    }

    /// Outsourced build function, to make error conversion easier later
    async fn build_helper(self) -> anyhow::Result<Request> {
        let recipe = self.recipe;
        // Don't let any sub-futures try to move the context
        let template_context = &self.template_context;

        let method = recipe.method.parse()?;

        // Build all the futures separately, then resolve them in parallel

        let url_future = recipe.url.render(template_context, "URL");

        // Build header map
        let headers_future = async {
            Ok(future::try_join_all(recipe.headers.iter().map(
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
                                    template_context,
                                    &format!("header {header}"),
                                )
                                .await?,
                        )?,
                    ))
                },
            ))
            .await?
            .into_iter()
            .collect::<HeaderMap>())
        };

        // Add query parameters
        let query_future = async {
            Ok(future::try_join_all(recipe.query.iter().map(
                |(k, v)| async move {
                    Ok::<_, anyhow::Error>((
                        k.clone(),
                        v.render(
                            template_context,
                            &format!("Query parameter {k}"),
                        )
                        .await?,
                    ))
                },
            ))
            .await?
            .into_iter()
            .collect::<IndexMap<String, String>>())
        };

        // Render the body
        let body_future = async {
            match &recipe.body {
                Some(body) => Ok(Some(
                    body.render(template_context, "body")
                        .await
                        .context("Body")?,
                )),
                None => Ok(None),
            }
        };

        // Zoooooooooooom!
        let (url, headers, query, body) =
            try_join!(url_future, headers_future, query_future, body_future,)?;

        info!(
            recipe_id = %recipe.id,
            "Built request from recipe",
        );

        Ok(Request {
            id: self.id,
            recipe_id: recipe.id.clone(),
            method,
            url,
            query,
            body,
            headers,
        })
    }
}
