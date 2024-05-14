//! HTTP-related data types

use crate::{
    collection::{ProfileId, RecipeId},
    http::{cereal, ContentType, ResponseContent},
    util::ResultExt,
};
use anyhow::Context;
use bytes::Bytes;
use bytesize::ByteSize;
use chrono::{DateTime, Duration, Utc};
use derive_more::{Display, From};
use mime::Mime;
use reqwest::{
    header::{self, HeaderMap},
    Method, StatusCode,
};
use serde::{Deserialize, Serialize};
use std::{
    fmt::{Debug, Write},
    sync::{Arc, OnceLock},
};
use thiserror::Error;
use tracing::error;
use url::Url;
use uuid::Uuid;

/// Unique ID for a single launched request
#[derive(
    Copy, Clone, Debug, Display, Eq, Hash, PartialEq, Serialize, Deserialize,
)]
pub struct RequestId(pub Uuid);

impl RequestId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for RequestId {
    fn default() -> Self {
        Self::new()
    }
}

/// A complete request+response pairing. This is generated by
/// [HttpEngine::send](super::HttpEngine::send) when a response is received
/// successfully for a sent request.
#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq))]
pub struct RequestRecord {
    /// ID to uniquely refer to this record. Useful for historical records.
    pub id: RequestId,
    /// What we said. Use an Arc so the view can hang onto it.
    pub request: Arc<Request>,
    /// What we heard. Use an Arc so the view can hang onto it.
    pub response: Arc<Response>,
    /// When was the request sent to the server?
    pub start_time: DateTime<Utc>,
    /// When did we finish receiving the *entire* response?
    pub end_time: DateTime<Utc>,
}

impl RequestRecord {
    /// Get the elapsed time for this request
    pub fn duration(&self) -> Duration {
        self.end_time - self.start_time
    }
}

/// A single instance of an HTTP request. There are a few reasons we need this
/// in addition to [reqwest::Request]:
/// - It stores additional Slumber-specific metadata
/// - It is serializable/deserializable, for database access
///
/// This intentionally does *not* implement `Clone`, because each request is
/// unique.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(test, derive(PartialEq))]
pub struct Request {
    /// Unique ID for this request. Private to prevent mutation
    pub id: RequestId,
    /// The profile used to render this request (for historical context)
    pub profile_id: Option<ProfileId>,
    /// The recipe used to generate this request (for historical context)
    pub recipe_id: RecipeId,

    #[serde(with = "cereal::serde_method")]
    pub method: Method,
    /// URL, including query params/fragment
    pub url: Url,
    #[serde(with = "cereal::serde_header_map")]
    pub headers: HeaderMap,
    /// Body content as bytes. This should be decoded as needed
    pub body: Option<Body>,
}

impl Request {
    /// Generate a cURL command equivalent to this request
    ///
    /// This only fails if one of the headers or body is binary and can't be
    /// converted to UTF-8.
    pub fn to_curl(&self) -> anyhow::Result<String> {
        let mut buf = String::new();

        // These writes are all infallible because we're writing to a string,
        // but use ? because it's shorter than unwrap().
        let method = &self.method;
        let url = &self.url;
        write!(&mut buf, "curl -X{method} --url '{url}'")?;

        for (header, value) in &self.headers {
            let value =
                value.to_str().context("Error decoding header value")?;
            write!(&mut buf, " --header '{header}: {value}'")?;
        }

        if let Some(body) = &self.body_str()? {
            write!(&mut buf, " --data '{body}'")?;
        }

        Ok(buf)
    }

    /// Get the body of the request, decoded as UTF-8. Returns an error if the
    /// body isn't valid UTF-8.
    pub fn body_str(&self) -> anyhow::Result<Option<&str>> {
        if let Some(body) = &self.body {
            Ok(Some(
                std::str::from_utf8(&body.data)
                    .context("Error decoding body")?,
            ))
        } else {
            Ok(None)
        }
    }
}

/// A resolved HTTP response, with all content loaded and ready to be displayed
/// to the user. A simpler alternative to [reqwest::Response], because there's
/// no way to access all resolved data on that type at once. Resolving the
/// response body requires moving the response.
///
/// This intentionally does not implement Clone, because responses could
/// potentially be very large.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(test, derive(PartialEq))]
pub struct Response {
    #[serde(with = "cereal::serde_status_code")]
    pub status: StatusCode,
    #[serde(with = "cereal::serde_header_map")]
    pub headers: HeaderMap,
    pub body: Body,
}

impl Response {
    /// Attempt to parse the body of this response, and store it in the body
    /// struct. If parsing fails, we'll store `None` instead.
    pub fn parse_body(&self) {
        let body = ContentType::parse_response(self)
            .context("Error parsing response body")
            .traced()
            .ok();
        // Store whether we succeeded or not, so we know not to try again
        if self.body.parsed.set(body).is_err() {
            // Unfortunately we don't have any helpful context to include here.
            // The body could potentially be huge so don't log it.
            error!("Response body parsed twice");
        }
    }

    /// Get a suggested file name for the content of this response. First we'll
    /// check the Content-Disposition header. If it's missing or doesn't have a
    /// file name, we'll check the Content-Type to at least guess at an
    /// extension.
    pub fn file_name(&self) -> Option<String> {
        self.headers
            .get(header::CONTENT_DISPOSITION)
            .and_then(|value| {
                // Parse header for the `filename="{}"` parameter
                // https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Content-Disposition
                let value = value.to_str().ok()?;
                value.split(';').find_map(|part| {
                    let (key, value) = part.trim().split_once('=')?;
                    if key == "filename" {
                        Some(value.trim_matches('"').to_owned())
                    } else {
                        None
                    }
                })
            })
            .or_else(|| {
                // Grab the extension from the Content-Type header. Don't use
                // self.conten_type() because we want to accept unknown types.
                let content_type = self.headers.get(header::CONTENT_TYPE)?;
                let mime: Mime = content_type.to_str().ok()?.parse().ok()?;
                Some(format!("data.{}", mime.subtype()))
            })
    }

    /// Get the content type of this response, based on the `Content-Type`
    /// header. Return `None` if the header is missing or an unknown type.
    pub fn content_type(&self) -> Option<ContentType> {
        ContentType::from_response(self).ok()
    }
}

/// HTTP request OR response body. Content is stored as bytes to support
/// non-text content. Should be converted to text only as needed
#[derive(Default, Deserialize)]
#[serde(from = "Bytes")] // Can't use into=Bytes because that requires cloning
pub struct Body {
    /// Raw body
    data: Bytes,
    /// For responses of a known content type, we can parse the body into a
    /// real data structure. This is populated *lazily*, i.e. on first request.
    /// Useful for filtering and prettification. We store `None` here if we
    /// tried and failed to parse, so that we know not to try again.
    #[serde(skip)]
    parsed: OnceLock<Option<Box<dyn ResponseContent>>>,
}

impl Body {
    pub fn new(data: Bytes) -> Self {
        Self {
            data,
            parsed: Default::default(),
        }
    }

    /// Raw content bytes
    pub fn bytes(&self) -> &[u8] {
        &self.data
    }

    /// Owned raw content bytes
    pub fn into_bytes(self) -> Bytes {
        self.data
    }

    /// Get bytes as text, if valid UTF-8
    pub fn text(&self) -> Option<&str> {
        std::str::from_utf8(&self.data).ok()
    }

    /// Get body size, in bytes
    pub fn size(&self) -> ByteSize {
        ByteSize(self.bytes().len() as u64)
    }

    /// Get the parsed version of this body. Must haved call
    /// [Response::parse_body] first to actually do the parse. Parsing has to
    /// be done on the parent because we don't have access to the `Content-Type`
    /// header here, which tells us how to parse.
    ///
    /// Return `None` if parsing either hasn't happened yet, or failed.
    pub fn parsed(&self) -> Option<&dyn ResponseContent> {
        self.parsed.get().and_then(Option::as_deref)
    }
}

impl Debug for Body {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Don't print the actual body because it could be huge
        f.debug_tuple("Body")
            .field(&format!("<{} bytes>", self.data.len()))
            .finish()
    }
}

impl From<Bytes> for Body {
    fn from(bytes: Bytes) -> Self {
        Self::new(bytes)
    }
}

impl Serialize for Body {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        // Serialize just the bytes, everything else is derived
        self.data.serialize(serializer)
    }
}

#[cfg(test)]
impl From<Vec<u8>> for Body {
    fn from(value: Vec<u8>) -> Self {
        Self::new(value.into())
    }
}

impl From<String> for Body {
    fn from(value: String) -> Self {
        Self::new(value.into())
    }
}

#[cfg(test)]
impl From<&str> for Body {
    fn from(value: &str) -> Self {
        Self::new(value.to_owned().into())
    }
}

#[cfg(test)]
impl PartialEq for Body {
    fn eq(&self, other: &Self) -> bool {
        // Ignore derived data
        self.data == other.data
    }
}

/// An error that can occur while *building* a request
#[derive(Debug, Error)]
#[error("Error building request {id}")]
pub struct RequestBuildError {
    /// ID of the failed request
    pub id: RequestId,
    /// There are a lot of different possible error types, so storing an anyhow
    /// is easiest
    #[source]
    pub error: anyhow::Error,
}

#[cfg(test)]
impl PartialEq for RequestBuildError {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.error.to_string() == other.error.to_string()
    }
}

/// An error that can occur during a request. This does *not* including building
/// errors.
#[derive(Debug, Error)]
#[error(
    "Error executing request for `{}` (request `{}`)",
    .request.recipe_id,
    .request.id,
)]
pub struct RequestError {
    /// Underlying error. This will always be a `reqwest::Error`, but wrapping
    /// it in anyhow makes it easier to render
    #[source]
    pub error: anyhow::Error,
    /// The request that caused all this ruckus
    pub request: Arc<Request>,
    /// When was the request launched?
    pub start_time: DateTime<Utc>,
    /// When did the error occur?
    pub end_time: DateTime<Utc>,
}

#[cfg(test)]
impl PartialEq for RequestError {
    fn eq(&self, other: &Self) -> bool {
        self.error.to_string() == other.error.to_string()
            && self.request == other.request
            && self.start_time == other.start_time
            && self.end_time == other.end_time
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::*;
    use indexmap::indexmap;
    use rstest::rstest;
    use serde_json::json;

    #[rstest]
    #[case::content_disposition(
        Response {
            headers: header_map(indexmap! {
                "content-disposition" => "form-data;name=\"field\"; filename=\"fish.png\"",
                "content-type" => "image/png",
            }),
            ..Response::factory()
        },
        Some("fish.png")
    )]
    #[case::content_type_known(
        Response {
            headers: header_map(indexmap! {
                "content-disposition" => "form-data",
                "content-type" => "application/json",
            }),
            ..Response::factory()
        },
        Some("data.json")
    )]
    #[case::content_type_unknown(
        Response {
            headers: header_map(indexmap! {
                "content-disposition" => "form-data",
                "content-type" => "image/jpeg",
            }),
            ..Response::factory()
        },
        Some("data.jpeg")
    )]
    #[case::none(Response::factory(), None)]
    fn test_file_name(
        #[case] response: Response,
        #[case] expected: Option<&str>,
    ) {
        assert_eq!(response.file_name().as_deref(), expected);
    }

    #[test]
    fn test_to_curl() {
        let headers = indexmap! {
            "accept" => "application/json",
            "content-type" => "application/json",
        };
        let body = json!({"data": "value"});
        let request = Request {
            method: Method::DELETE,
            headers: header_map(headers),
            body: Some(serde_json::to_vec(&body).unwrap().into()),
            ..Request::factory()
        };

        assert_eq!(
            request.to_curl().unwrap(),
            "curl -XDELETE --url 'http://localhost/url' \
            --header 'accept: application/json' \
            --header 'content-type: application/json' \
            --data '{\"data\":\"value\"}'"
        );
    }
}
