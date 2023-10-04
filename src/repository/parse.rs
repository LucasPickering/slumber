use crate::{http::Response, util::ResultExt};
use anyhow::Context;
use reqwest::header::{self, HeaderValue};

/// A parsed response body. We have a set number of supported content types that
/// we know how to parse using various serde implementations.
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
    /// Parse the body of a response, based on its content-type header
    pub fn parse(response: &Response) -> anyhow::Result<ParsedBody> {
        let content_type = response
            .headers
            .get(header::CONTENT_TYPE)
            .map(HeaderValue::as_bytes);
        let result: anyhow::Result<Self> = try {
            match content_type {
                Some(b"application/json") => {
                    let json_value = serde_json::from_str(&response.body)?;
                    Self::Json(json_value)
                }
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

    // TODO add a type-stated parse_as
}
