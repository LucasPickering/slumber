use mime::{APPLICATION, JSON, Mime};
use reqwest::header::{self, HeaderMap, HeaderValue};
use slumber_config::MimeOverrideMap;

/// A known MIME type, for which we support prettification and syntax
/// highlighting
#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq)]
pub enum SyntaxType {
    Json,
}

impl SyntaxType {
    /// Get a known content type from a pre-parsed MIME type
    ///
    /// Return `None` if the MIME type is unknown. `mime_overrides` is a mapping
    /// of MIME transformations to apply *before* parsing the MIME into a syntax
    /// type. This is sourced from the config.
    pub fn from_mime(
        mime_overrides: &MimeOverrideMap,
        mime: &Mime,
    ) -> Option<Self> {
        // Apply MIME override first
        let mime = mime_overrides.get(mime);
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

    /// Parse the content type from the `Content-Type` header
    ///
    /// Return `None` if the `Content-Type` header is missing, contains an
    /// invalid MIME value, or an unknown MIME type.
    pub fn from_headers(
        mime_overrides: &MimeOverrideMap,
        headers: &HeaderMap,
    ) -> Option<Self> {
        let header_value = headers
            .get(header::CONTENT_TYPE)
            .map(HeaderValue::as_bytes)?;
        let header_value = std::str::from_utf8(header_value).ok()?;
        let mime: Mime = header_value.parse().ok()?;
        Self::from_mime(mime_overrides, &mime)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mime::APPLICATION_JSON;
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    #[rstest]
    #[case::json("application/json", Some(SyntaxType::Json))]
    #[case::json_with_metadata(
        // Test extra metadata in the content-type header
        "application/json; charset=utf-8; boundary=asdf",
        Some(SyntaxType::Json)
    )]
    // Test extended MIME type
    #[case::json_extended("application/geo+json", Some(SyntaxType::Json))]
    #[case::mime_override("text/fake", Some(SyntaxType::Json))]
    // Error cases
    #[case::error_json_empty_extension("application/+json", None)]
    #[case::error_unknown("text/html", None)]
    fn test_from_mime(
        #[case] mime_type: Mime,
        #[case] expected: Option<SyntaxType>,
    ) {
        let overrides =
            MimeOverrideMap::from_iter([("text/fake", APPLICATION_JSON)]);
        assert_eq!(SyntaxType::from_mime(&overrides, &mime_type), expected);
    }

    #[rstest]
    #[case::json(Some("application/json"), Some(SyntaxType::Json))]
    // Error cases
    #[case::error_missing(None, None)]
    #[case::error_invalid(Some("json"), None)]
    #[case::error_whitespace(Some("application/ +json"), None)]
    fn test_from_headers(
        #[case] content_type_header: Option<&'static str>,
        #[case] expected: Option<SyntaxType>,
    ) {
        let headers = content_type_header
            .into_iter()
            .map(|value| {
                (header::CONTENT_TYPE, HeaderValue::from_static(value))
            })
            .collect::<HeaderMap>();
        assert_eq!(
            SyntaxType::from_headers(&MimeOverrideMap::default(), &headers),
            expected
        );
    }

    /// Test prettification
    #[rstest]
    #[case::json(
        SyntaxType::Json,
        r#"{"hello": "goodbye"}"#,
        Some("{\n  \"hello\": \"goodbye\"\n}")
    )]
    // Invalid JSON => no pretty value available
    #[case::invalid_json(SyntaxType::Json, r#"{"hello": "goodbye""#, None)]
    fn test_prettyify(
        #[case] content_type: SyntaxType,
        #[case] body: &str,
        #[case] expected: Option<&str>,
    ) {
        assert_eq!(content_type.prettify(body).as_deref(), expected);
    }
}
