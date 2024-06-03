//! Utilities for parsing response bodies into a variety of known content types.
//! Each supported content type has its own struct which implements
//! [ResponseContent]. If you want to parse as a statically known content type,
//! just use that struct. If you just need to refer to the content _type_, and
//! not a value, use [ContentType]. If you want to parse dynamically based on
//! the response's metadata, use [ContentType::parse_response].

use crate::{http::ResponseRecord, util::Mapping};
use anyhow::{anyhow, Context};
use derive_more::{Deref, Display, From};
use mime::{Mime, APPLICATION, JSON};
use reqwest::header::{self, HeaderValue};
use serde::{Deserialize, Serialize};
use std::{borrow::Cow, ffi::OsStr, fmt::Debug, path::Path};

/// All supported content types. Each variant should have a corresponding
/// implementation of [ResponseContent].
///
/// Each content type is can be referred to in a few ways:
/// - Its serialization string, which is only used within Slumber (e.g. in the
///   collection model)
/// - Its MIME type
/// - Its file extension(s)
///
/// For the serialization string, obviously use serde. For the others, use
/// the corresponding methods/associated functions.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentType {
    Json,
}

impl ContentType {
    /// File extensions for each content type
    const EXTENSIONS: Mapping<'static, ContentType> =
        Mapping::new(&[(Self::Json, &["json"])]);

    /// Get the file extension associated with this content type. For content
    /// types that have multiple common extensions (e.g. `image/jpeg` has `jpeg`
    /// and `jpg`), return whichever is defined first in the mapping.
    pub fn extension(&self) -> &'static str {
        Self::EXTENSIONS.get_label(*self)
    }

    /// Parse the value of the content-type header and map it to a known content
    /// type
    fn from_mime(mime_type: &str) -> anyhow::Result<Self> {
        let mime_type: Mime = mime_type
            .parse()
            .with_context(|| format!("Invalid content type `{mime_type}`"))?;

        let suffix = mime_type.suffix().map(|name| name.as_str());
        match (mime_type.type_(), mime_type.subtype(), suffix) {
            // JSON has a lot of extended types that follow the pattern
            // "application/*+json", match those too
            (APPLICATION, JSON, _) | (APPLICATION, _, Some("json")) => {
                Ok(Self::Json)
            }
            _ => Err(anyhow!("Unknown content type `{mime_type}`")),
        }
    }

    /// Guess content type from a file path based on its extension
    pub fn from_path(path: &Path) -> anyhow::Result<Self> {
        let extension = path
            .extension()
            .and_then(OsStr::to_str)
            .ok_or_else(|| anyhow!("Path {path:?} has no extension"))?;
        Self::EXTENSIONS
            .get(extension)
            .ok_or_else(|| anyhow!("Unknown extension `{extension}`"))
    }

    /// Parse the content type from a response's `Content-Type` header
    pub fn from_response(response: &ResponseRecord) -> anyhow::Result<Self> {
        let header_value = response
            .headers
            .get(header::CONTENT_TYPE)
            .map(HeaderValue::as_bytes)
            .ok_or_else(|| anyhow!("Response has no content-type header"))?;
        let header_value = std::str::from_utf8(header_value)
            .context("content-type header is not valid utf-8")?;
        Self::from_mime(header_value)
    }

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

    /// Helper for parsing the body of a response. Use
    /// [ResponseRecord::parse_body] for external usage.
    pub(super) fn parse_response(
        response: &ResponseRecord,
    ) -> anyhow::Result<Box<dyn ResponseContent>> {
        let content_type = Self::from_response(response)?;
        content_type.parse_content(response.body.bytes())
    }
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

/// Parse JSON content
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{assert_err, Factory};
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
        assert_eq!(ContentType::from_mime(mime_type).unwrap(), expected);
    }

    /// Test invalid/unknown MIME types
    #[rstest]
    #[case::invalid("json", "Invalid content type")]
    #[case::json_empty_extension("application/+json", "Unknown content type")]
    #[case::whitespace("application/ +json", "Invalid content type")]
    #[case::unknown("text/html", "Unknown content type")]
    fn test_try_from_mime_error(
        #[case] mime_type: &str,
        #[case] expected_error: &str,
    ) {
        assert_err!(ContentType::from_mime(mime_type), expected_error);
    }

    #[test]
    fn test_from_extension() {
        assert_eq!(
            ContentType::from_path(Path::new("turbo.json")).unwrap(),
            ContentType::Json
        );

        // Errors
        assert_err!(
            ContentType::from_path(Path::new("no_extension")),
            "no extension"
        );
        assert_err!(
            ContentType::from_path(Path::new("turbo.ohno")),
            "Unknown extension `ohno`"
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
        #[case] body: &str,
        #[case] expected: T,
    ) {
        let response = ResponseRecord {
            headers: headers(content_type),
            body: body.into(),
            ..ResponseRecord::factory(())
        };
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
        "Invalid content type `bad-header`"
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
        #[case] body: &str,
        #[case] expected_error: &str,
    ) {
        let headers = match content_type {
            Some(content_type) => headers(content_type),
            None => HeaderMap::new(),
        };
        let response = ResponseRecord {
            headers,
            body: body.into(),
            ..ResponseRecord::factory(())
        };
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
