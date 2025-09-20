use crate::{
    collection::Authentication,
    http::{FormPart, HttpMethod, RenderedBody},
};
use anyhow::Context;
use reqwest::header::{self, HeaderMap, HeaderName, HeaderValue};
use slumber_template::{Stream, StreamSource};
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
    pub fn headers(mut self, headers: &HeaderMap) -> anyhow::Result<Self> {
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
    ) -> anyhow::Result<Self> {
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

    /// Add a body to the command
    pub fn body(mut self, body: RenderedBody) -> anyhow::Result<Self> {
        match body {
            RenderedBody::Raw(Stream::Value(value)) => {
                let bytes = value.into_bytes();
                let body = as_text(&bytes)?;
                self.push(["--data".into(), format!("'{body}'").into()]);
            }
            RenderedBody::Raw(Stream::Stream {
                source: StreamSource::File { path },
                ..
            }) => {
                // Stream the file
                self.push([
                    "--data".into(),
                    format!("'@{path}'", path = path.to_string_lossy()).into(),
                ]);
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
                for (field, part) in form {
                    let value = match &part {
                        FormPart::Bytes(bytes) => as_text(bytes)?,
                        // Use curl's file path syntax
                        FormPart::File(path) => {
                            let path = path.to_string_lossy();
                            &format!("@{path}")
                        }
                    };
                    self.push([
                        "-F".into(),
                        format!("'{field}={value}'").into(),
                    ]);
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
fn as_text(bytes: &[u8]) -> anyhow::Result<&str> {
    std::str::from_utf8(bytes)
        .context("curl command generation only supports text values")
}
