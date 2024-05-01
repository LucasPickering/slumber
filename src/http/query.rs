//! Utilities for querying HTTP response data

use crate::http::ResponseContent;
use derive_more::{Display, FromStr};
use serde::{Deserialize, Serialize};
use serde_json_path::{ExactlyOneError, JsonPath};
use std::borrow::Cow;
use thiserror::Error;

/// A wrapper around a JSONPath. This combines some common behavior, and will
/// make it easy to swap out the query language in the future if necessary.
#[derive(Clone, Debug, Display, FromStr, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Query(JsonPath);

#[derive(Debug, Error)]
pub enum QueryError {
    /// Got either 0 or 2+ results for JSON path query
    #[error("Expected exactly one result from query")]
    InvalidResult {
        #[from]
        #[source]
        error: ExactlyOneError,
    },
}

impl Query {
    /// Apply a query to some content, returning the result in the original
    /// format. This will convert to a common format, apply the query, then
    /// convert back.
    pub fn query(
        &self,
        value: &dyn ResponseContent,
    ) -> Box<dyn ResponseContent> {
        let content_type = value.content_type();
        let json_value = value.to_json();
        // We have to clone all the elements to put them into a JSON array
        let queried = serde_json::Value::Array(
            self.0.query(&json_value).into_iter().cloned().collect(),
        );
        content_type.parse_json(Cow::Owned(queried))
    }

    /// Apply a query to some content, returning a string. The query should
    /// return a single result. If it's a scalar, that will be stringified. If
    /// it's an array/object, it'll be converted back into its input format,
    /// then stringified.
    pub fn query_to_string(
        &self,
        value: &dyn ResponseContent,
    ) -> Result<String, QueryError> {
        let content_type = value.content_type();

        // All content types get converted to JSON for querying, then converted
        // back. This is fucky but we need *some* common format
        let json_value = value.to_json();
        let queried = self.0.query(&json_value).exactly_one()?;

        // If we got a scalar value, use that. Otherwise convert back to the
        // input content type to re-stringify
        let stringified = match queried {
            serde_json::Value::Null => "".into(),
            serde_json::Value::Number(n) => n.to_string(),
            serde_json::Value::Bool(b) => b.to_string(),
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
                content_type.parse_json(Cow::Borrowed(queried)).to_string()
            }
        };

        Ok(stringified)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{http::Json, test_util::*};
    use rstest::rstest;
    use serde_json::json;

    #[rstest]
    #[case::root("$", json(json!({"test": "hi!"})), r#"{"test":"hi!"}"#)]
    #[case::string("$.test", json(json!({"test": "hi!"})), "hi!")]
    #[case::int("$.test", json(json!({"test": 3})), "3")]
    #[case::bool("$.test", json(json!({"test": true})), "true")]
    fn test_query_to_string(
        #[case] query: &str,
        #[case] content: Box<dyn ResponseContent>,
        #[case] expected: &str,
    ) {
        let query = Query::from_str(query).unwrap();
        let out = query.query_to_string(&*content).unwrap();
        assert_eq!(out, expected);
    }

    #[rstest]
    #[case::too_many_results("$[*]", json(json!([1, 2])), "Expected exactly one result")]
    #[case::no_results("$[*]", json(json!([])), "Expected exactly one result")]
    fn test_query_to_string_error(
        #[case] query: &str,
        #[case] content: Box<dyn ResponseContent>,
        #[case] expected_err: &str,
    ) {
        let query = Query::from_str(query).unwrap();
        assert_err!(query.query_to_string(&*content), expected_err);
    }

    /// Helper to create JSON content
    fn json(value: serde_json::Value) -> Box<dyn ResponseContent> {
        Box::new(Json::from(value))
    }
}
