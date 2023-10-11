//! Utilities for parsing response bodies into a variety of known content types.
//! This uses a pseudo-type state pattern. Each supported content type has its
//! own struct which implements [ContentType]. Then a wrapper enum [ParsedBody]
//! holds the actual parsed content. [ParsedBody] supports fallible downcasting
//! to get a specific content type if necessary.
//!
//! There is probably a way to improve this. I originally went with this
//! architecture when it was all getting shoved behind an `Arc` in the cache,
//! but that shouldn't be necessary anymore.

use crate::{http::Response, util::ResultExt};
use anyhow::{anyhow, Context};
use reqwest::header::{self, HeaderValue};

/// A parsed response body. We have a set number of supported content types that
/// we know how to parse using various serde implementations.
///
/// Use [Response::parse] to obtain one of these.
#[derive(Debug)]
pub enum ParsedBody {
    Json(serde_json::Value),
    /// Response has an unknown `content-type` so we can't attempt a parse.
    /// The `content-type` will be included if possible. It will be missing if
    /// the header is missing or the value is not valid UTF-8.
    UnknownContentType {
        content_type: Option<String>,
    },
}

impl ParsedBody {
    fn content_type_display(&self) -> &str {
        match self {
            ParsedBody::Json(_) => Json::HEADER,
            ParsedBody::UnknownContentType { content_type } => {
                content_type.as_deref().unwrap_or("<unknown>")
            }
        }
    }
}

/// A response content type that we know how to parse.
pub trait ContentType {
    /// Value of the `content-type` header identifying this content type
    const HEADER: &'static str;
    /// Useful for pattern matching
    const HEADER_BYTES: &'static [u8] = Self::HEADER.as_bytes();

    /// The type that the body will parse into
    type Value;

    /// Parse the response body
    fn parse(body: &str) -> anyhow::Result<Self::Value>;

    fn from_parsed_body(parsed_body: &ParsedBody) -> Option<&Self::Value>;
}

pub struct Json;
impl ContentType for Json {
    const HEADER: &'static str = "application/json";
    type Value = serde_json::Value;

    fn parse(body: &str) -> anyhow::Result<Self::Value> {
        Ok(serde_json::from_str(body)?)
    }

    fn from_parsed_body(parsed_body: &ParsedBody) -> Option<&Self::Value> {
        match parsed_body {
            ParsedBody::Json(value) => Some(value),
            _ => None,
        }
    }
}

impl ParsedBody {
    /// Parse the body of a response, based on its `content-type` header. Use
    /// [Response::parse] to parse from outside the `http` module.
    pub(super) fn parse(response: &Response) -> anyhow::Result<ParsedBody> {
        let body = &response.body;
        let result: anyhow::Result<Self> = try {
            match content_type(response) {
                Some(Json::HEADER_BYTES) => Self::Json(Json::parse(body)?),

                // Content type is either missing or unknown
                Some(content_type) => Self::UnknownContentType {
                    // Try to parse the content, but don't try too hard
                    content_type: String::from_utf8(content_type.to_owned())
                        .ok(),
                },
                None => Self::UnknownContentType { content_type: None },
            }
        };
        result.context("Error parsing response body").traced()
    }

    /// Attempt to downcast the parsed body to a particular content type. If
    /// content type doesn't match, return an error.
    ///
    /// Generally using this does mean you'll have to parse the response even if
    /// it isn't the content type you're looking for, but that provides
    /// simplicity and will cache the parsed body for other purposes too.
    pub fn as_content_type<CT: ContentType>(
        &self,
    ) -> anyhow::Result<&CT::Value> {
        CT::from_parsed_body(self).ok_or_else(|| {
            anyhow!(
                "Expected content type {}, but response was {}",
                CT::HEADER,
                self.content_type_display()
            )
        })
    }
}

/// Get the value of the `content-type` header for a response
fn content_type(response: &Response) -> Option<&[u8]> {
    response
        .headers
        .get(header::CONTENT_TYPE)
        .map(HeaderValue::as_bytes)
}
