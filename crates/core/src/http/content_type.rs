use mime::{APPLICATION, JSON, Mime};
use reqwest::header::{self, HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use std::{fmt::Debug, str::Utf8Error};
use thiserror::Error;

/// A known content type, for which we support prettification and syntax
/// highlighting
#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentType {
    Json,
}

impl ContentType {
    /// Get a known content type from a pre-parsed MIME type
    ///
    /// Return `Err` if the MIME type is unknown.
    pub fn try_from_mime(mime: &Mime) -> Result<Self, ContentTypeError> {
        let suffix = mime.suffix().map(|name| name.as_str());
        match (mime.type_(), mime.subtype(), suffix) {
            // JSON has a lot of extended types that follow the pattern
            // "application/*+json", match those too
            (APPLICATION, JSON, _) | (APPLICATION, _, Some("json")) => {
                Ok(Self::Json)
            }
            _ => Err(ContentTypeError::MimeUnknown(mime.clone())),
        }
    }

    /// Parse the content type from the `Content-Type` header
    ///
    /// Return `Err` if the `Content-Type` header is missing, contains an
    /// invalid MIME value, or an unknown MIME type.
    pub fn try_from_headers(
        headers: &HeaderMap,
    ) -> Result<Self, ContentTypeError> {
        let header_value = headers
            .get(header::CONTENT_TYPE)
            .map(HeaderValue::as_bytes)
            .ok_or(ContentTypeError::HeaderMissing)?;
        let header_value = std::str::from_utf8(header_value)
            .map_err(ContentTypeError::HeaderInvalid)?;
        let mime: Mime = header_value.parse().map_err(|_| {
            ContentTypeError::MimeInvalid(header_value.to_owned())
        })?;
        Self::try_from_mime(&mime)
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
}

/// Error parsing a content type or extracting the content type from a response
#[derive(Debug, Error)]
pub enum ContentTypeError {
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
    use rstest::rstest;
    use slumber_util::assert_result;

    #[rstest]
    #[case::json("application/json", Ok(ContentType::Json))]
    #[case::json_with_metadata(
        // Test extra metadata in the content-type header
        "application/json; charset=utf-8; boundary=asdf",
        Ok(ContentType::Json)
    )]
    // Test extended MIME type
    #[case::json_extended("application/geo+json", Ok(ContentType::Json))]
    // Error cases
    #[case::error_json_empty_extension(
        "application/+json",
        Err("Unknown content type")
    )]
    #[case::error_unknown("text/html", Err("Unknown content type"))]
    fn test_try_from_mime(
        #[case] mime_type: Mime,
        #[case] expected: Result<ContentType, &str>,
    ) {
        assert_result(ContentType::try_from_mime(&mime_type), expected);
    }

    #[rstest]
    #[case::json(Some("application/json"), Ok(ContentType::Json))]
    // Error cases
    #[case::error_missing(None, Err("Response has no Content-Type header"))]
    #[case::error_invalid(Some("json"), Err("Invalid content type"))]
    #[case::error_whitespace(
        Some("application/ +json"),
        Err("Invalid content type")
    )]
    fn test_try_from_headers(
        #[case] content_type_header: Option<&'static str>,
        #[case] expected: Result<ContentType, &str>,
    ) {
        let headers = content_type_header
            .into_iter()
            .map(|value| {
                (header::CONTENT_TYPE, HeaderValue::from_static(value))
            })
            .collect::<HeaderMap>();
        assert_result(ContentType::try_from_headers(&headers), expected);
    }

    /// Test prettification
    #[rstest]
    #[case::json(
        ContentType::Json,
        r#"{"hello": "goodbye"}"#,
        Some("{\n  \"hello\": \"goodbye\"\n}")
    )]
    // Invalid JSON => no pretty value available
    #[case::invalid_json(ContentType::Json, r#"{"hello": "goodbye""#, None)]
    fn test_prettyify(
        #[case] content_type: ContentType,
        #[case] body: &str,
        #[case] expected: Option<&str>,
    ) {
        assert_eq!(content_type.prettify(body).as_deref(), expected);
    }
}
