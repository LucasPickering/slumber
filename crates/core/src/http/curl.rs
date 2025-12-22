use crate::{
    collection::Authentication,
    http::{BodyStream, HttpMethod, RenderedBody, RequestBuildErrorKind},
};
use bytes::BytesMut;
use futures::TryStreamExt;
use itertools::Itertools;
use reqwest::header::{self, HeaderMap, HeaderName, HeaderValue};
use slumber_template::StreamSource;

/// Builder pattern for constructing cURL commands from a recipe
pub struct CurlBuilder {
    /// Command argument groups. Each group contains related args (e.g., flag
    /// and its value) that should stay on the same line.
    groups: Vec<Vec<String>>,
}

impl CurlBuilder {
    /// Start building a new cURL command for an HTTP method
    pub fn new(method: HttpMethod) -> Self {
        Self {
            groups: vec![vec!["curl".into(), format!("-X{method}")]],
        }
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
        // Add to the first group, so it goes on the same line as the method
        self.groups[0].extend(["--url".into(), format!("'{url}'")]);
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
        self.groups
            .push(vec!["--header".into(), format!("'{name}: {value}'")]);
        Ok(self)
    }

    /// Add an authentication scheme to the command
    pub fn authentication(
        mut self,
        authentication: &Authentication<String>,
    ) -> Self {
        match authentication {
            Authentication::Basic { username, password } => {
                self.groups.push(vec![
                    "--user".into(),
                    format!(
                        "'{username}:{password}'",
                        password = password.as_deref().unwrap_or_default()
                    ),
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
                self.groups.push(vec!["--data".into(), format!("'{body}'")]);
            }
            // We know how to stream files to curl
            RenderedBody::Stream(BodyStream {
                source: Some(StreamSource::File { path }),
                ..
            }) => {
                // Stream the file
                self.groups.push(vec![
                    "--data".into(),
                    format!("'@{path}'", path = path.to_string_lossy()),
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
                self.groups.push(vec!["--data".into(), format!("'{body}'")]);
            }
            RenderedBody::Json(json) => {
                self.groups
                    .push(vec!["--json".into(), format!("'{json:#}'")]);
            }
            // Use the first-class form support where possible
            RenderedBody::FormUrlencoded(form) => {
                for (field, value) in form {
                    self.groups.push(vec![
                        "--data-urlencode".into(),
                        format!("'{field}={value}'"),
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
                    self.groups.push(vec!["-F".into(), argument]);
                }
            }
        }
        Ok(self)
    }

    /// Finalize and return the command
    pub fn build(self) -> String {
        /// Between args in the same group
        const ARG_SEPARATOR: &str = " ";
        /// Between separate groups
        const GROUP_SEPARATOR: &str = " \\\n  ";

        self.groups
            .into_iter()
            .map(|group| group.join(ARG_SEPARATOR))
            .join(GROUP_SEPARATOR)
    }
}

/// Convert bytes to text, or return an error if it's not UTF-8
fn as_text(bytes: &[u8]) -> Result<&str, RequestBuildErrorKind> {
    std::str::from_utf8(bytes).map_err(RequestBuildErrorKind::CurlInvalidUtf8)
}
