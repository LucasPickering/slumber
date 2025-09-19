use crate::{
    collection::Authentication,
    http::{FormPart, HttpMethod, RenderedBody},
};
use anyhow::Context;
use reqwest::header::{self, HeaderMap, HeaderName, HeaderValue};
use slumber_template::{Stream, StreamMetadata};
use std::fmt::Write;

/// Builder pattern for constructing cURL commands from a recipe
pub struct CurlBuilder {
    // TODO build as a vec of args instead of a string?
    command: String,
}

impl CurlBuilder {
    /// Start building a new cURL command for an HTTP method
    pub fn new(method: HttpMethod) -> Self {
        Self {
            command: format!("curl -X{method}"),
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
        write!(&mut self.command, " --url '{url}'").unwrap();
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
        write!(&mut self.command, " --header '{name}: {value}'").unwrap();
        Ok(self)
    }

    /// Add an authentication scheme to the command
    pub fn authentication(
        mut self,
        authentication: &Authentication<String>,
    ) -> Self {
        match authentication {
            Authentication::Basic { username, password } => {
                write!(
                    &mut self.command,
                    " --user '{username}:{password}'",
                    password = password.as_deref().unwrap_or_default()
                )
                .unwrap();
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
                write!(&mut self.command, " --data '{body}'").unwrap();
            }
            RenderedBody::Raw(Stream::Stream {
                metadata: StreamMetadata::File { path },
                ..
            }) => {
                // Stream the file
                write!(
                    &mut self.command,
                    " --data '@{path}'",
                    path = path.to_string_lossy()
                )
                .unwrap();
            }
            RenderedBody::Json(json) => {
                write!(&mut self.command, " --json '{json}'").unwrap();
            }
            // Use the first-class form support where possible
            RenderedBody::FormUrlencoded(form) => {
                for (field, value) in form {
                    write!(
                        &mut self.command,
                        " --data-urlencode '{field}={value}'"
                    )
                    .unwrap();
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
                    write!(&mut self.command, " -F '{field}={value}'").unwrap();
                }
            }
        }
        Ok(self)
    }

    /// Finalize and return the command
    pub fn build(self) -> String {
        self.command
    }
}

/// Convert bytes to text, or return an error if it's not UTF-8
fn as_text(bytes: &[u8]) -> anyhow::Result<&str> {
    std::str::from_utf8(bytes)
        .context("curl command generation only supports text values")
}
