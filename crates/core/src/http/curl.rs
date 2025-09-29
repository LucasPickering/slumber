use crate::{
    collection::Authentication,
    http::{BodyStream, HttpMethod, RenderedBody, RequestBuildErrorKind},
};
use bytes::BytesMut;
use futures::TryStreamExt;
use reqwest::header::{self, HeaderMap, HeaderName, HeaderValue};
use slumber_template::StreamSource;
use std::borrow::Cow;

/// Builder pattern for constructing cURL commands from a recipe
pub struct CurlBuilder {
    /// Command arguments. Built up as a list then joined into a string
    command: Vec<Cow<'static, str>>,
}

impl CurlBuilder {
    /// Start building a new cURL command for an HTTP method
    pub fn new(method: HttpMethod) -> Self {
        let mut slf = Self { command: vec![] };
        slf.push(["curl".into(), format!("-X{method}").into()]);
        slf
    }

    /// Add the URL, with query parameters, to the command
    pub fn url(
        mut self,
        mut url: reqwest::Url,
        query: &[(String, String)],
    ) -> Self {
        // Add a query string. The empty check prevents a dangling ? if there
        // are no query params
        if !query.is_empty() {
            url.query_pairs_mut().extend_pairs(query);
        }
        self.push(["--url".into(), format!("'{url}'").into()]);
        self
    }

    /// Add an entire map of headers to the command
    pub fn headers(
        mut self,
        headers: &HeaderMap,
    ) -> Result<Self, RequestBuildErrorKind> {
        for (name, value) in headers {
            self = self.header(name, value)?;
        }
        Ok(self)
    }

    /// Add a header to the command
    pub fn header(
        mut self,
        name: &HeaderName,
        value: &HeaderValue,
    ) -> Result<Self, RequestBuildErrorKind> {
        let value = as_text(value.as_bytes())?;
        self.push(["--header".into(), format!("'{name}: {value}'").into()]);
        Ok(self)
    }

    /// Add an authentication scheme to the command
    pub fn authentication(
        mut self,
        authentication: &Authentication<String>,
    ) -> Self {
        match authentication {
            Authentication::Basic { username, password } => {
                self.push([
                    "--user".into(),
                    format!(
                        "'{username}:{password}'",
                        password = password.as_deref().unwrap_or_default()
                    )
                    .into(),
                ]);
                self
            }
            Authentication::Bearer { token } => self
                .header(
                    &header::AUTHORIZATION,
                    // The token is base64-encoded so we know it's valid
                    &HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
                )
                // Failure isn't possible because we know the value is UTF-8
                .unwrap(),
        }
    }

    /// Add a body to the command. This is async because the body may be an
    /// stream that needs to be resolved.
    pub async fn body(
        mut self,
        body: RenderedBody,
    ) -> Result<Self, RequestBuildErrorKind> {
        match body {
            RenderedBody::Raw(bytes) => {
                let body = as_text(&bytes)?;
                self.push(["--data".into(), format!("'{body}'").into()]);
            }
            // We know how to stream files to curl
            RenderedBody::Stream(BodyStream {
                source: Some(StreamSource::File { path }),
                ..
            }) => {
                // Stream the file
                self.push([
                    "--data".into(),
                    format!("'@{path}'", path = path.to_string_lossy()).into(),
                ]);
            }
            // Any other type of has to be resolved eagerly since curl
            // doesn't support them natively
            RenderedBody::Stream(stream) => {
                let bytes = stream
                    .stream
                    .try_collect::<BytesMut>()
                    .await
                    .map_err(RequestBuildErrorKind::BodyStream)?;
                let body = as_text(&bytes)?;
                self.push(["--data".into(), format!("'{body}'").into()]);
            }
            RenderedBody::Json(json) => {
                self.push(["--json".into(), format!("'{json}'").into()]);
            }
            // Use the first-class form support where possible
            RenderedBody::FormUrlencoded(form) => {
                for (field, value) in form {
                    self.push([
                        "--data-urlencode".into(),
                        format!("'{field}={value}'").into(),
                    ]);
                }
            }
            RenderedBody::FormMultipart(form) => {
                for (field, stream) in form {
                    let argument = if let Some(StreamSource::File { path }) =
                        stream.source
                    {
                        // Files can be passed directly to curl
                        let path = path.to_string_lossy();
                        format!("'{field}=@{path}'")
                    } else {
                        let bytes = stream
                            .stream
                            .try_collect::<BytesMut>()
                            .await
                            .map_err(RequestBuildErrorKind::BodyStream)?;
                        let text = as_text(&bytes)?;
                        format!("'{field}={text}'")
                    };
                    self.push(["-F".into(), argument.into()]);
                }
            }
        }
        Ok(self)
    }

    /// Finalize and return the command
    pub fn build(self) -> String {
        self.command.join(" ")
    }

    /// Push arguments onto the list
    fn push<const N: usize>(&mut self, arguments: [Cow<'static, str>; N]) {
        self.command.extend(arguments);
    }
}

/// Convert bytes to text, or return an error if it's not UTF-8
fn as_text(bytes: &[u8]) -> Result<&str, RequestBuildErrorKind> {
    std::str::from_utf8(bytes).map_err(RequestBuildErrorKind::CurlInvalidUtf8)
}
