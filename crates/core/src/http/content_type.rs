//! Utilities for parsing response bodies into a variety of known content types.
//! Each supported content type has its own struct which implements
//! [ResponseContent]. If you want to parse as a statically known content type,
//! just use that struct. If you just need to refer to the content _type_, and
//! not a value, use [ContentType]. If you want to parse dynamically based on
//! the response's metadata, use [ContentType::from_headers] and
//! [ContentType::parse_content].

use derive_more::{Deref, Display, From};
use mime::{APPLICATION, JSON, Mime};
use reqwest::header::{self, HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use std::{borrow::Cow, fmt::Debug, str::Utf8Error};
use thiserror::Error;

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
#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentType {
    Json,
}

impl ContentType {
    /// Parse a MIME string and map it to a known content type
    fn parse_mime(mime_type: &str) -> Result<Self, ContentTypeError> {
        let mime: Mime = mime_type
            .parse()
            .map_err(|_| ContentTypeError::MimeInvalid(mime_type.to_owned()))?;
        Self::from_mime(&mime).ok_or(ContentTypeError::MimeUnknown(mime))
    }

    /// Get a known content type from a pre-parsed MIME type. Return `None` if
    /// the MIME type isn't supported.
    pub fn from_mime(mime: &Mime) -> Option<Self> {
        let suffix = mime.suffix().map(|name| name.as_str());
        match (mime.type_(), mime.subtype(), suffix) {
            // JSON has a lot of extended types that follow the pattern
            // "application/*+json", match those too
            (APPLICATION, JSON, _) | (APPLICATION, _, Some("json")) => {
                Some(Self::Json)
            }
            _ => None,
        }
    }

    /// Get the MIME for this content type
    pub fn to_mime(&self) -> Mime {
        match self {
            ContentType::Json => mime::APPLICATION_JSON,
        }
    }

    /// Parse the content type from the `Content-Type` header
    pub fn from_headers(headers: &HeaderMap) -> Result<Self, ContentTypeError> {
        let header_value = headers
            .get(header::CONTENT_TYPE)
            .map(HeaderValue::as_bytes)
            .ok_or(ContentTypeError::HeaderMissing)?;
        let header_value = std::str::from_utf8(header_value)
            .map_err(ContentTypeError::HeaderInvalid)?;
        Self::parse_mime(header_value)
    }

    /// Parse some content of this type. Return a dynamically dispatched content
    /// object.
    pub fn parse_content(
        self,
        content: &[u8],
    ) -> Result<Box<dyn ResponseContent>, ContentTypeError> {
        match self {
            Self::Json => Ok(Box::new(Json::parse(content)?)),
        }
    }

    /// Convert content from JSON into this format. Valid JSON should be valid
    /// in any other format too, so this is infallible.
    pub fn parse_json(
        self,
        content: serde_json::Value,
    ) -> Box<dyn ResponseContent> {
        match self {
            Self::Json => Box::new(Json(content)),
        }
    }

    /// Make a response body look pretty. If the input isn't valid for this
    /// content type, return `None`
    pub fn prettify(&self, body: &str) -> Option<String> {
        match self {
            ContentType::Json => {
                // The easiest way to prettify is to parse and restringify.
                // There's definitely faster ways that don't require building
                // the whole data structure in memory, but not via serde
                if let Ok(parsed) =
                    serde_json::from_str::<serde_json::Value>(body)
                {
                    // serde_json shouldn't fail serializing its own Value type
                    serde_json::to_string_pretty(&parsed).ok()
                } else {
                    // Not valid JSON
                    None
                }
            }
        }
    }

    /// Stringify a single JSON value into this format
    pub fn value_to_string(self, value: &serde_json::Value) -> String {
        match self {
            ContentType::Json => match value {
                serde_json::Value::Null => String::new(),
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            },
        }
    }

    /// Stringify a list of JSON values into this format
    pub fn vec_to_string(self, values: &Vec<&serde_json::Value>) -> String {
        match self {
            ContentType::Json => serde_json::to_string(&values).unwrap(),
        }
    }
}

/// A response content type that we know how to parse. This is defined as a
/// trait rather than an enum because it breaks apart the logic more clearly.
pub trait ResponseContent: Debug + Display + Send + Sync {
    /// Get the type of this content
    fn content_type(&self) -> ContentType;

    /// Parse the response body as this type
    fn parse(body: &[u8]) -> Result<Self, ContentTypeError>
    where
        Self: Sized;

    /// Convert the content to JSON. JSON is the common language used for
    /// querying internally, so everything needs to be convertible to/from JSON.
    fn to_json(&self) -> Cow<'_, serde_json::Value>;

    /// Facilitate downcasting generic parsed bodies to concrete types for tests
    #[cfg(test)]
    fn as_any(&self) -> &dyn std::any::Any;
}

/// JSON content type
#[derive(Debug, Display, Deref, From, PartialEq)]
pub struct Json(serde_json::Value);

impl ResponseContent for Json {
    fn content_type(&self) -> ContentType {
        ContentType::Json
    }

    fn parse(body: &[u8]) -> Result<Self, ContentTypeError> {
        Ok(Self(serde_json::from_slice(body)?))
    }

    fn to_json(&self) -> Cow<'_, serde_json::Value> {
        Cow::Borrowed(&self.0)
    }

    #[cfg(test)]
    fn as_any(&self) -> &dyn std::any::Any {
        self as &dyn std::any::Any
    }
}

/// Error parsing a content type or extracting the content type from a response
#[derive(Debug, Error)]
pub enum ContentTypeError {
    /// Error parsing content as JSON
    #[error(transparent)]
    Json(#[from] serde_json::Error),

    /// Input was not a valid MIME type
    #[error("Invalid content type `{0}`")]
    MimeInvalid(String),

    /// Input was a valid MIME type but not one that maps to a known content
    /// type
    #[error("Unknown content type `{0}`")]
    MimeUnknown(Mime),

    /// Response doesn't have a `Content-Type` header
    #[error("Response has no Content-Type header")]
    HeaderMissing,

    /// Response has a `Content-Type` header but it's not UTF-8
    #[error("Content-Type header is not valid UTF-8")]
    HeaderInvalid(#[source] Utf8Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::ResponseRecord;
    use reqwest::header::{
        CONTENT_TYPE, HeaderMap, HeaderValue, InvalidHeaderValue,
    };
    use rstest::rstest;
    use serde_json::json;
    use slumber_util::{Factory, assert_err};
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
        assert_eq!(ContentType::parse_mime(mime_type).unwrap(), expected);
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
        assert_err!(ContentType::parse_mime(mime_type), expected_error);
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
        let content_type =
            ContentType::from_headers(&response.headers).unwrap();
        assert_eq!(
            content_type
                .parse_content(response.body.bytes())
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
    #[case::no_content_type(
        None::<&str>,
        "",
        "Response has no Content-Type header",
    )]
    #[case::unknown_content_type(
        Some("bad-header"),
        "",
        "Invalid content type `bad-header`"
    )]
    #[case::invalid_header_utf8(
        Some(b"\xc3\x28".as_slice()),
        "",
        "Content-Type header is not valid UTF-8",
    )]
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
        let result = ContentType::from_headers(&response.headers).and_then(
            |content_type| content_type.parse_content(response.body.bytes()),
        );
        assert_err!(result, expected_error);
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
