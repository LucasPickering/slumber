//! This whole module is basically a wrapper around reqwest to make it more
//! ergnomic for our needs

use crate::{config::RequestRecipe, template::TemplateValues};
use reqwest::{header::HeaderMap, Client, Method, StatusCode};
use std::sync::Arc;
use tokio::{sync::RwLock, task::JoinHandle};

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
    /// Resolved response, or an error. Since this gets populated
    /// asynchronously, we need to store it behind a lock
    pub response: Arc<RwLock<Option<reqwest::Result<Response>>>>,
}

/// A resolved HTTP response, with all content loaded and ready to be displayed
/// in the UI
#[derive(Debug)]
pub struct Response {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub content: String,
}

impl HttpEngine {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }

    /// Instantiate a request from a recipe, using values from the given
    /// environment to render templated strings
    pub fn build_request(
        &self,
        recipe: &RequestRecipe,
        template_values: &TemplateValues,
    ) -> anyhow::Result<Request> {
        let method = recipe.method.render(template_values)?.parse()?;
        let url = recipe.url.render(template_values)?;
        Ok(Request {
            method,
            url,
            headers: HeaderMap::new(), // TODO
            response: Arc::new(RwLock::new(None)),
        })
    }

    /// Launch a request in a spawned task. The response will be stored with
    /// the request
    pub fn send_request(&self, request: &Request) -> JoinHandle<()> {
        // Convert to reqwest's request format
        let reqwest_request = self
            .client
            .request(request.method.clone(), request.url.clone())
            .build()
            .expect("Error building HTTP request");

        // Launch the request in a task
        let response_box = Arc::clone(&request.response);
        // Client is safe to clone, it uses Arc internally
        // https://docs.rs/reqwest/0.11.20/reqwest/struct.Client.html
        let client = self.client.clone();
        tokio::spawn(async move {
            // Execute the request and get all response metadata/content
            let result: reqwest::Result<Response> = async {
                let reqwest_response = client.execute(reqwest_request).await?;

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
            .await;

            // Store the result with the request
            *response_box.write().await = Some(result);
        })
    }
}
