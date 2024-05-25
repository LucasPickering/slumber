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
//! +--------+          +--------------+
//! | future | -error-> | RequestError |
//! +--------+          +--------------+
//!      |
//!   success
//!      |
//!      v
//! +---------------+
//! | RequestRecord |
//! +---------------+

mod cereal;
mod content_type;
mod query;
mod record;

pub use content_type::*;
pub use query::*;
pub use record::*;

use crate::{
    collection::{self, Authentication, Method, Recipe},
    config::Config,
    db::CollectionDatabase,
    template::{Template, TemplateContext},
    util::ResultExt,
};
use anyhow::Context;
use base64::{prelude::BASE64_STANDARD, write::EncoderWriter};
use chrono::Utc;
use futures::future;
use indexmap::IndexMap;
use reqwest::{
    header::{self, HeaderMap, HeaderName, HeaderValue},
    Client,
};
use std::{collections::HashSet, future::Future, io::Write, sync::Arc};
use tokio::try_join;
use tracing::{debug, info, info_span};
use url::Url;

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

    /// Launch an HTTP request. Upon completion, it will automatically be
    /// registered in the database for posterity.
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
        database: &CollectionDatabase,
        request: Arc<Request>,
    ) -> Result<RequestRecord, RequestError> {
        let id = request.id;

        let span = info_span!("HTTP request", request_id = %id);
        span.in_scope(|| async move {
            // This start time will be accurate because the request doesn't
            // launch until this whole future is awaited

            // Technically the elapsed time will include the conversion time,
            // but that should be extremely minimal compared to network IO
            let start_time = Utc::now();
            let result = self.send_request_helper(&request).await;
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
                        response: Arc::new(response),
                        start_time,
                        end_time,
                    };

                    // Error here should *not* kill the request
                    let _ = database.insert_request(&record);
                    Ok(record)
                }
                Err(error) => Err(RequestError {
                    request,
                    start_time,
                    end_time,
                    error: error.into(),
                })
                .traced(),
            }
        })
        .await
    }

    /// An exact encapsulation of the "request". The execution of this function
    /// is synonymous with a request's elapsed time.
    async fn send_request_helper(
        &self,
        request: &Request,
    ) -> reqwest::Result<Response> {
        // Convert to reqwest format as part of the execution. This means
        // certain builder errors will show up as "request" errors which is
        // janky, but reqwest already doesn't report some builder erorrs until
        // you execute the request, and this is much easier than frontloading
        // the conversion during the build process.
        let reqwest_request = self.convert_request(request)?;

        // If the user wants to ignore cert errors on this host, use the client
        // that's set up for that
        let host = reqwest_request.url().host_str().unwrap_or_default();
        let client = if self.danger_hostnames.contains(host) {
            &self.danger_client
        } else {
            &self.client
        };

        let reqwest_response = client.execute(reqwest_request).await?;
        // Load the full response and convert it to our format
        self.convert_response(reqwest_response).await
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
            .request(request.method.clone(), request.url.clone())
            .headers(request.headers.clone());

        // Add body
        if let Some(body) = &request.body {
            request_builder = request_builder.body(body.bytes().to_owned());
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
        let body = response.bytes().await?.into();

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
    recipe: Recipe,
    options: RecipeOptions,
}

/// OPtions for modifying a recipe during a build. This is helpful for applying
/// temporary modifications made by the user. By providing this in a separate
/// struct, we prevent the need to clone, modify, and pass recipes everywhere.
/// Recipes could be very large so cloning may be expensive, and this options
/// layer makes the available modifications clear and restricted.
#[derive(Clone, Debug, Default)]
#[cfg_attr(test, derive(PartialEq))]
pub struct RecipeOptions {
    /// Which headers should be excluded? A blacklist allows the default to be
    /// "include all".
    pub disabled_headers: HashSet<String>,
    /// Which query parameters should be excluded?  A blacklist allows the
    /// default to be "include all".
    pub disabled_query_parameters: HashSet<String>,
}

impl RequestBuilder {
    /// Instantiate new request builder for the given recipe. Use [Self::build]
    /// to build it.
    ///
    /// This needs an owned recipe and context so they can be moved into a
    /// subtask for the build.
    pub fn new(recipe: Recipe, options: RecipeOptions) -> Self {
        debug!(recipe_id = %recipe.id, "Building request from recipe");
        let request_id = RequestId::new();

        Self {
            id: request_id,
            recipe,
            options,
        }
    }

    /// The unique ID generated for this request, which can be used to track it
    /// throughout its life cycle
    pub fn id(&self) -> RequestId {
        self.id
    }

    /// Build the request. This is async because templated values may require IO
    /// or other async actions.
    pub async fn build(
        self,
        template_context: &TemplateContext,
    ) -> Result<Request, RequestBuildError> {
        self.apply_error(
            self.render_request(template_context),
            template_context,
        )
        .await
    }

    /// Build just a request's URL
    pub async fn build_url(
        self,
        template_context: &TemplateContext,
    ) -> Result<Url, RequestBuildError> {
        self.apply_error(self.render_url(template_context), template_context)
            .await
    }

    /// Build just a request's body
    pub async fn build_body(
        self,
        template_context: &TemplateContext,
    ) -> Result<Option<Body>, RequestBuildError> {
        self.apply_error(self.render_body(template_context), template_context)
            .await
    }

    /// Wrapper to apply a helpful error around some request build step
    async fn apply_error<T>(
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

    /// Render the entire request
    async fn render_request(
        &self,
        template_context: &TemplateContext,
    ) -> anyhow::Result<Request> {
        // Render everything in parallel
        let (url, headers, body) = try_join!(
            self.render_url(template_context),
            self.render_headers(template_context),
            self.render_body(template_context),
        )?;

        info!(
            recipe_id = %self.recipe.id,
            "Built request from recipe",
        );

        Ok(Request {
            id: self.id,
            profile_id: template_context.selected_profile.clone(),
            recipe_id: self.recipe.id.clone(),
            method: self.recipe.method.into(),
            url,
            headers,
            body,
        })
    }

    /// Render URL, including query params
    async fn render_url(
        &self,
        template_context: &TemplateContext,
    ) -> anyhow::Result<Url> {
        // Shitty try block
        let (mut url, query) = try_join!(
            async {
                let url = self
                    .recipe
                    .url
                    .render(template_context)
                    .await
                    .context("Error rendering URL")?;
                url.parse::<Url>()
                    .with_context(|| format!("Invalid URL: `{url}`"))
            },
            self.render_query(template_context)
        )?;

        // Join query into URL. if check prevents bare ? for empty query
        if !query.is_empty() {
            url.query_pairs_mut().extend_pairs(&query);
        }

        Ok(url)
    }

    /// Render query key=value params
    async fn render_query(
        &self,
        template_context: &TemplateContext,
    ) -> anyhow::Result<IndexMap<String, String>> {
        let iter = self
            .recipe
            .query
            .iter()
            // Filter out disabled params
            .filter(|(param, _)| {
                !self.options.disabled_query_parameters.contains(*param)
            })
            .map(|(k, v)| async move {
                Ok::<_, anyhow::Error>((
                    k.clone(),
                    v.render(template_context).await.context(format!(
                        "Error rendering query parameter `{k}`"
                    ))?,
                ))
            });
        Ok(future::try_join_all(iter)
            .await?
            .into_iter()
            .collect::<IndexMap<String, String>>())
    }

    /// Render all headers. This will also render authentication and merge it
    /// into the headers
    async fn render_headers(
        &self,
        template_context: &TemplateContext,
    ) -> anyhow::Result<HeaderMap> {
        // Render base headers
        let iter = self
            .recipe
            .headers
            .iter()
            // Filter out disabled headers
            .filter(|(header, _)| {
                !self.options.disabled_headers.contains(*header)
            })
            .map(move |(header, value_template)| {
                self.render_header(template_context, header, value_template)
            });
        let mut headers = future::try_join_all(iter)
            .await?
            .into_iter()
            .collect::<HeaderMap>();

        // Render auth method and modify headers accordingly
        if let Some(authentication) = &self.recipe.authentication {
            headers.insert(
                header::AUTHORIZATION,
                self.render_authentication(template_context, authentication)
                    .await?,
            );
        }

        Ok(headers)
    }

    /// Render authentication and return a value for the Authorization header
    async fn render_authentication(
        &self,
        template_context: &TemplateContext,
        authentication: &Authentication,
    ) -> anyhow::Result<HeaderValue> {
        let mut header_value = match authentication {
            collection::Authentication::Basic { username, password } => {
                // Encode as `username:password | base64`
                // https://swagger.io/docs/specification/authentication/basic-authentication/
                let (username, password) = try_join!(
                    async {
                        username
                            .render(template_context)
                            .await
                            .context("Error rendering username")
                    },
                    async {
                        Template::render_opt(
                            password.as_ref(),
                            template_context,
                        )
                        .await
                        .context("Error rendering password")
                    },
                )?;

                let mut buf = b"Basic ".to_vec();
                {
                    let mut encoder =
                        EncoderWriter::new(&mut buf, &BASE64_STANDARD);
                    let _ = write!(encoder, "{username}:");
                    if let Some(password) = password {
                        let _ = write!(encoder, "{password}");
                    }
                }
                HeaderValue::from_bytes(&buf)
                    .context("Error encoding basic authentication credentials")
            }

            collection::Authentication::Bearer(token) => {
                let token = token
                    .render(template_context)
                    .await
                    .context("Error rendering bearer token")?;
                format!("Bearer {token}")
                    .try_into()
                    .context("Error encoding bearer token")
            }
        }?;
        header_value.set_sensitive(true);
        Ok(header_value)
    }

    /// Render a single key/value header
    async fn render_header(
        &self,
        template_context: &TemplateContext,
        header: &str,
        value_template: &Template,
    ) -> anyhow::Result<(HeaderName, HeaderValue)> {
        let value = value_template
            .render(template_context)
            .await
            .context(format!("Error rendering header `{header}`"))?;
        // Strip leading/trailing line breaks because they're going to
        // trigger a validation error and are probably a mistake. This
        // is a balance between convenience and
        // explicitness
        let value = value.trim_matches(|c| c == '\n' || c == '\r');
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

    async fn render_body(
        &self,
        template_context: &TemplateContext,
    ) -> anyhow::Result<Option<Body>> {
        let body =
            Template::render_opt(self.recipe.body.as_ref(), template_context)
                .await
                .context("Error rendering body")?;
        Ok(body.map(Body::from))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        collection::{Authentication, Collection, Profile},
        test_util::{header_map, Factory},
    };
    use indexmap::indexmap;
    use pretty_assertions::assert_eq;
    use reqwest::Method;
    use rstest::rstest;
    use std::collections::HashMap;

    #[tokio::test]
    async fn test_build_request() {
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
        let context = TemplateContext {
            collection: Collection {
                profiles: indexmap! {profile_id.clone() => profile},
                ..Collection::factory(())
            },
            selected_profile: Some(profile_id.clone()),
            ..TemplateContext::factory(())
        };
        let recipe = Recipe {
            method: "POST".parse().unwrap(),
            url: "{{host}}/users/{{user_id}}".into(),
            query: indexmap! {
                "mode".into() => "{{mode}}".into(),
                "fast".into() => "true".into(),
            },
            headers: indexmap! {
                "Accept".into() => "application/json".into(),
                "Content-Type".into() => "application/json".into(),
            },
            body: Some("{\"group_id\":\"{{group_id}}\"}".into()),
            ..Recipe::factory(())
        };
        let recipe_id = recipe.id.clone();

        let builder = RequestBuilder::new(recipe, RecipeOptions::default());
        let request = builder.build(&context).await.unwrap();

        let expected_headers = indexmap! {
            "content-type" => "application/json",
            "accept" => "application/json",
        };

        assert_eq!(
            request,
            Request {
                id: request.id,
                profile_id: Some(profile_id),
                recipe_id,
                method: Method::POST,
                url: "http://localhost/users/1?mode=sudo&fast=true"
                    .parse()
                    .unwrap(),
                body: Some(Vec::from(b"{\"group_id\":\"3\"}").into()),
                headers: header_map(expected_headers),
            }
        );
    }

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
        let context = TemplateContext {
            collection: Collection {
                profiles: indexmap! {profile_id.clone() => profile},
                ..Collection::factory(())
            },
            selected_profile: Some(profile_id.clone()),
            ..TemplateContext::factory(())
        };
        let recipe = Recipe {
            authentication: Some(authentication),
            ..Recipe::factory(())
        };
        let recipe_id = recipe.id.clone();

        let builder = RequestBuilder::new(recipe, RecipeOptions::default());
        let request = builder.build(&context).await.unwrap();

        let expected_headers: HashMap<String, String> =
            [("authorization", expected_header)]
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();

        assert_eq!(
            request,
            Request {
                id: request.id,
                profile_id: Some(profile_id),
                recipe_id,
                method: Method::GET,
                url: "http://localhost/url".parse().unwrap(),
                headers: (&expected_headers).try_into().unwrap(),
                body: None,
            }
        );
    }

    #[tokio::test]
    async fn test_disable_headers_and_query_params() {
        let context = TemplateContext::factory(());
        let recipe = Recipe {
            query: indexmap! {
                "mode".into() => "sudo".into(),
                "fast".into() => "true".into(),
            },
            headers: indexmap! {
                "Accept".into() => "application/json".into(),
                "Content-Type".into() => "application/json".into(),
            },
            ..Recipe::factory(())
        };
        let recipe_id = recipe.id.clone();

        let builder = RequestBuilder::new(
            recipe,
            RecipeOptions {
                disabled_headers: ["Content-Type".to_owned()].into(),
                disabled_query_parameters: ["fast".to_owned()].into(),
            },
        );
        let request = builder.build(&context).await.unwrap();

        let expected_headers: HashMap<String, String> =
            [("accept", "application/json")]
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();

        assert_eq!(
            request,
            Request {
                id: request.id,
                profile_id: None,
                recipe_id,
                method: Method::GET,
                url: "http://localhost/url?mode=sudo".parse().unwrap(),
                headers: (&expected_headers).try_into().unwrap(),
                body: None,
            }
        );
    }
}
