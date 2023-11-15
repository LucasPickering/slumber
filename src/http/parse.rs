//! Utilities for parsing response bodies into a variety of known content types.
//! Each supported content type has its own struct which implements
//! [ContentType]. If you want to parse as a statically known content type, just
//! use that struct. If you're want to parse dynamically based on the response's
//! metadata, use [parse_body].

use crate::http::Response;
use anyhow::{anyhow, Context};
use derive_more::Deref;
use std::fmt::Debug;

/// A response content type that we know how to parse.
pub trait ContentType: Debug {
    /// Parse the response body as this type
    fn parse(body: &str) -> anyhow::Result<Self>
    where
        Self: Sized;

    /// Prettify a parsed body into something the user will really like. Once
    /// a response is parsed, prettification is infallible. Could be slow
    /// though!
    fn prettify(&self) -> String;

    /// Facilitate downcasting generic parsed bodies to concrete types for tests
    #[cfg(test)]
    fn as_any(&self) -> &dyn std::any::Any;
}

#[derive(Debug, Deref, PartialEq)]
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

    #[cfg(test)]
    fn as_any(&self) -> &dyn std::any::Any {
        self as &dyn std::any::Any
    }
}

/// Helper for parsing the body of a response. Use [Response::parse_body] for
/// external usage.
pub(super) fn parse_body(
    response: &Response,
) -> anyhow::Result<Box<dyn ContentType>> {
    let body = &response.body;
    match get_content_type(response)? {
        Json::HEADER => Ok(Box::new(Json::parse(body.text())?)),
        other => Err(anyhow!("Response has unknown content-type `{other}`",)),
    }
}

/// Parse the content type from a response's headers
fn get_content_type(response: &Response) -> anyhow::Result<&str> {
    // If the header value isn't utf-8, we're hosed
    let header_value = std::str::from_utf8(
        response
            .content_type()
            .ok_or_else(|| anyhow!("Response has no content-type header"))?,
    )
    .context("content-type header is not valid utf-8")?;

    // Remove extra metadata from the header. It feels like there should be a
    // helper for this in hyper or reqwest but I couldn't find it.
    // https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Content-Type
    Ok(header_value
        .split_once(';')
        .map(|t| t.0)
        .unwrap_or(header_value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{factory::_Factori_Builder_Response, util::assert_err};
    use factori::create;
    use reqwest::header::{
        HeaderMap, HeaderValue, InvalidHeaderValue, CONTENT_TYPE,
    };
    use rstest::rstest;
    use serde_json::json;
    use std::ops::Deref;

    /// Test all content types
    #[rstest]
    #[case(
        "application/json",
        "{\"hello\": \"goodbye\"}",
        Json(json!({"hello": "goodbye"}))
    )]
    #[case(
        // Test extra metadata in the content-type header
        "application/json; charset=utf-8; boundary=asdf",
        "{\"hello\": \"goodbye\"}",
        Json(json!({"hello": "goodbye"}))
    )]
    fn test_parse_body<T: ContentType + PartialEq + 'static>(
        #[case] content_type: &str,
        #[case] body: String,
        #[case] expected: T,
    ) {
        let response = create!(
            Response, headers: headers(content_type), body: body.into()
        );
        assert_eq!(
            parse_body(&response)
                .unwrap()
                .deref()
                // Downcast the result to desired type
                .as_any()
                .downcast_ref::<T>()
                .unwrap(),
            &expected
        );
    }

    /// Test various failure cases
    #[rstest]
    #[case(None::<&str>, "", "no content-type header")]
    #[case(Some("bad-header"), "", "unknown content-type")]
    #[case(Some(b"\xc3\x28".as_slice()), "", "not valid utf-8")]
    #[case(Some("application/json"), "not json!", "expected ident")]
    fn test_parse_body_error<
        T: TryInto<HeaderValue, Error = InvalidHeaderValue>,
    >(
        #[case] content_type: Option<T>,
        #[case] body: String,
        #[case] expected_error: &str,
    ) {
        let headers = match content_type {
            Some(content_type) => headers(content_type),
            None => HeaderMap::new(),
        };
        let response = create!(Response, headers: headers, body: body.into());
        assert_err!(parse_body(&response), expected_error);
    }

    /// Create header map with the given value for the content-type header
    fn headers(
        content_type: impl TryInto<HeaderValue, Error = InvalidHeaderValue>,
    ) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, content_type.try_into().unwrap());
        headers
    }
}
