//! Utilities for parsing response bodies into a variety of known content types.
//! Each supported content type has its own struct which implements
//! [ContentType]. If you want to parse as a statically known content type, just
//! use that struct. If you're want to parse dynamically based on the response's
//! metadata, use [parse_body].

use crate::http::Response;
use anyhow::{anyhow, Context};
use derive_more::Deref;

/// A response content type that we know how to parse.
pub trait ContentType {
    /// Parse the response body as this type
    fn parse(body: &str) -> anyhow::Result<Self>
    where
        Self: Sized;

    /// Prettify a parsed body into something the user will really like. Once
    /// a response is parsed, prettification is infallible. Could be slow
    /// though!
    fn prettify(&self) -> String;
}

#[derive(Debug, Deref)]
pub struct Json(serde_json::Value);

impl Json {
    pub const HEADER: &'static str = "application/json";
}

impl ContentType for Json {
    fn parse(body: &str) -> anyhow::Result<Self> {
        Ok(Self(serde_json::from_str(body)?))
    }

    fn prettify(&self) -> String {
        // serde_json can't fail serializing its own Value type
        serde_json::to_string_pretty(&self.0).unwrap()
    }
}

/// Helper for parsing the body of a response. Use [Response::parse_body] for
/// external usage.
pub(super) fn parse_body(
    response: &Response,
) -> anyhow::Result<Box<dyn ContentType>> {
    // Convert the content type to utf-8
    let content_type = std::str::from_utf8(
        response
            .content_type()
            .ok_or_else(|| anyhow!("Response has no content-type header"))?,
    )
    .context("content-type header is not valid utf-8")?;

    let body = &response.body;
    match content_type {
        Json::HEADER => Ok(Box::new(Json::parse(body.text())?)),
        other => Err(anyhow!("Response has unknown content-type {other:?}",)),
    }
}
