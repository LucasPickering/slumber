//! Utilities for parsing response bodies into a variety of known content types.
//! Each supported content type has its own struct which implements
//! [ResponseContent]. If you want to parse as a statically known content type,
//! just use that struct. If you just need to refer to the content _type_, and
//! not a value, use [ContentType]. If you want to parse dynamically based on
//! the response's metadata, use [ContentType::parse_response].

use crate::http::Response;
use anyhow::{anyhow, Context};
use derive_more::{Deref, Display, From};
use regex::Regex;
use serde::{de::IntoDeserializer, Deserialize, Serialize};
use std::{borrow::Cow, ffi::OsStr, fmt::Debug, path::Path, sync::OnceLock};

/// All supported content types. Each variant should have a corresponding
/// implementation of [ResponseContent].
///
/// Serialization/deserialization of this only uses the short name. To parse
/// a MIME type (from an HTTP header), use [Self::from_response]. This is to
/// prevent accidentally supporting invalid MIME types.
#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(test, derive(PartialEq))]
#[serde(rename_all = "snake_case")]
pub enum ContentType {
    // Primary serialization string here should match the string we expect
    // users to enter in their collection file for manual overrides, i.e. the
    // most obvious/user-friendly value. MIME types are implemented
    // separately.
    Json,
}

/// A response content type that we know how to parse. This is defined as a
/// trait rather than an enum because it breaks apart the logic more clearly.
pub trait ResponseContent: Debug + Display + Send + Sync {
    /// Get the type of this content
    fn content_type(&self) -> ContentType;

    /// Parse the response body as this type
    fn parse(body: &[u8]) -> anyhow::Result<Self>
    where
        Self: Sized;

    /// Prettify a parsed body into something the user will really like. Once
    /// a response is parsed, prettification is infallible. Could be slow
    /// though!
    fn prettify(&self) -> String;

    /// Convert the content to JSON. JSON is the common language used for
    /// querying intenally, so everything needs to be convertible to/from JSON.
    fn to_json(&self) -> Cow<'_, serde_json::Value>;

    /// Facilitate downcasting generic parsed bodies to concrete types for tests
    #[cfg(test)]
    fn as_any(&self) -> &dyn std::any::Any;
}

#[derive(Debug, Display, Deref, From, PartialEq)]
pub struct Json(serde_json::Value);

impl ResponseContent for Json {
    fn content_type(&self) -> ContentType {
        ContentType::Json
    }

    fn parse(body: &[u8]) -> anyhow::Result<Self> {
        Ok(Self(serde_json::from_slice(body)?))
    }

    fn prettify(&self) -> String {
        // serde_json can't fail serializing its own Value type
        serde_json::to_string_pretty(&self.0).unwrap()
    }

    fn to_json(&self) -> Cow<'_, serde_json::Value> {
        Cow::Borrowed(&self.0)
    }

    #[cfg(test)]
    fn as_any(&self) -> &dyn std::any::Any {
        self as &dyn std::any::Any
    }
}

impl ContentType {
    /// Parse some content of this type. Return a dynamically dispatched content
    /// object.
    pub fn parse_content(
        self,
        content: &[u8],
    ) -> anyhow::Result<Box<dyn ResponseContent>> {
        match self {
            Self::Json => Ok(Box::new(Json::parse(content)?)),
        }
    }

    /// Convert content from JSON into this format. Valid JSON should be valid
    /// in any other format too, so this is infallible. This takes a `Cow`
    /// because some formats may need an owned JSON value while others may not.
    /// You should pass an owned value if you have it, but it's not necessary.
    pub fn parse_json(
        self,
        content: Cow<'_, serde_json::Value>,
    ) -> Box<dyn ResponseContent> {
        match self {
            Self::Json => Box::new(Json(content.into_owned())),
        }
    }

    /// Helper for parsing the body of a response. Use [Response::parse_body]
    /// for external usage.
    pub(super) fn parse_response(
        response: &Response,
    ) -> anyhow::Result<Box<dyn ResponseContent>> {
        let content_type = Self::from_response(response)?;
        content_type.parse_content(response.body.bytes())
    }

    /// Parse the content type from a file's extension
    pub fn from_extension(path: &Path) -> anyhow::Result<Self> {
        let extension = path
            .extension()
            .and_then(OsStr::to_str)
            .ok_or_else(|| anyhow!("Path {path:?} has no extension"))?;
        // Lean on serde for parsing
        ContentType::deserialize(extension.into_deserializer())
            .map_err(serde::de::value::Error::into)
    }

    /// Parse the content type from a response's `Content-Type` header
    pub fn from_response(response: &Response) -> anyhow::Result<Self> {
        // If the header value isn't utf-8, we're hosed
        let header_value = response
            .content_type()
            .ok_or_else(|| anyhow!("Response has no content-type header"))?;
        let header_value = std::str::from_utf8(header_value)
            .context("content-type header is not valid utf-8")?;
        Self::from_header(header_value)
    }

    /// Parse the value of the content-type header and map it to a known content
    /// type
    fn from_header(header_value: &str) -> anyhow::Result<Self> {
        // unstable: use LazyLock https://github.com/rust-lang/rust/pull/121377
        static JSON_REGEX: OnceLock<Regex> = OnceLock::new();

        // Remove extra metadata from the header. It feels like there should be
        // a helper for this in hyper or reqwest but I couldn't find it.
        // https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Content-Type
        let content_type = header_value
            .split_once(';')
            .map(|t| t.0)
            .unwrap_or(header_value);

        let regex = JSON_REGEX.get_or_init(|| {
            Regex::new("^application/(\\w+\\+)?json$").unwrap()
        });

        if regex.is_match(content_type) {
            Ok(Self::Json)
        } else {
            Err(anyhow!("Unknown content type {header_value:?}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{test_util::_Factori_Builder_Response, util::assert_err};
    use factori::create;
    use reqwest::header::{
        HeaderMap, HeaderValue, InvalidHeaderValue, CONTENT_TYPE,
    };
    use rstest::rstest;
    use serde_json::json;
    use std::ops::Deref;

    /// Test all content types and their variants
    #[rstest]
    #[case::json("application/json", ContentType::Json)]
    #[case::json_with_metadata(
        // Test extra metadata in the content-type header
        "application/json; charset=utf-8; boundary=asdf",
        ContentType::Json
    )]
    // Test extended MIME type
    #[case::json_extended("application/geo+json", ContentType::Json)]
    fn test_try_from_mime(
        #[case] mime_type: &str,
        #[case] expected: ContentType,
    ) {
        assert_eq!(ContentType::from_header(mime_type).unwrap(), expected);
    }

    /// Test invalid/unknown MIME types
    #[rstest]
    #[case::invalid("json")] // Bare types not supported
    #[case::json_empty_extension("application/+json")]
    #[case::whitespace("application/ +json")] // Spaces are bad!
    #[case::unknown("text/html")]
    fn test_try_from_mime_error(#[case] mime_type: &str) {
        assert_err!(
            ContentType::from_header(mime_type),
            "Unknown content type"
        );
    }

    #[test]
    fn test_from_extension() {
        assert_eq!(
            ContentType::from_extension(Path::new("turbo.json")).unwrap(),
            ContentType::Json
        );

        // Errors
        assert_err!(
            ContentType::from_extension(Path::new("no_extension")),
            "no extension"
        );
        assert_err!(
            ContentType::from_extension(Path::new("turbo.ohno")),
            "unknown variant `ohno`"
        )
    }

    /// Test all content types
    #[rstest]
    #[case::json(
        "application/json",
        "{\"hello\": \"goodbye\"}",
        Json(json!({"hello": "goodbye"}))
    )]
    fn test_parse_body<T: ResponseContent + PartialEq + 'static>(
        #[case] content_type: &str,
        #[case] body: String,
        #[case] expected: T,
    ) {
        let response = create!(
            Response, headers: headers(content_type), body: body.into()
        );
        assert_eq!(
            ContentType::parse_response(&response)
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
    #[case::no_content_type(None::<&str>, "", "no content-type header")]
    #[case::unknown_content_type(
        Some("bad-header"),
        "",
        "Unknown content type \"bad-header\""
    )]
    #[case::invalid_header_utf8(Some(b"\xc3\x28".as_slice()), "", "not valid utf-8")]
    #[case::invalid_content(
        Some("application/json"),
        "not json!",
        "expected ident"
    )]
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
        assert_err!(ContentType::parse_response(&response), expected_error);
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
