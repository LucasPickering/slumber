//! Utilities for querying HTTP response data

use crate::{collection::SelectorMode, http::content_type::ResponseContent};
use derive_more::{Display, FromStr};
use serde::{Deserialize, Serialize};
use serde_json_path::{ExactlyOneError, JsonPath};
use thiserror::Error;

/// A wrapper around a JSONPath. This combines some common behavior, and will
/// make it easy to swap out the query language in the future if necessary.
#[derive(Clone, Debug, Display, FromStr, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Query(JsonPath);

impl Query {
    /// Apply a query to some content, returning the result in the original
    /// format. This will convert to a common format (JSON), apply the query,
    /// then convert back.
    pub fn query_content(
        &self,
        value: &dyn ResponseContent,
    ) -> Box<dyn ResponseContent> {
        let content_type = value.content_type();
        let json_value = value.to_json();
        // We have to clone all the elements to put them into a JSON array
        let queried = serde_json::Value::Array(
            self.0.query(&json_value).into_iter().cloned().collect(),
        );
        content_type.parse_json(queried)
    }

    /// Apply a query to some content, returning a string. The query should
    /// return a single result. If it's a scalar, that will be stringified. If
    /// it's an array/object, it'll be converted back into its input format,
    /// then stringified.
    pub fn query_to_string(
        &self,
        mode: SelectorMode,
        value: &dyn ResponseContent,
    ) -> Result<String, QueryError> {
        let content_type = value.content_type();

        // All content types get converted to JSON for querying, then converted
        // back. This is fucky but we need *some* common format
        let json_value = value.to_json();
        let node_list = self.0.query(&json_value);

        let stringified = match mode {
            SelectorMode::Auto => match node_list.len() {
                0 => return Err(QueryError::NoResults),
                1 => content_type.value_to_string(node_list.first().unwrap()),
                2.. => content_type.vec_to_string(&node_list.all()),
            },
            SelectorMode::Single => {
                content_type.value_to_string(node_list.exactly_one()?)
            }
            SelectorMode::Array => content_type.vec_to_string(&node_list.all()),
        };

        Ok(stringified)
    }
}

#[cfg(test)]
impl From<&str> for Query {
    fn from(value: &str) -> Self {
        Self(value.parse().unwrap())
    }
}

/// A remapping of [serde_json_path::ExactlyOneError]. This is a simplified
/// version that implements `Clone`, which makes it easier to use within
/// template errors.
#[derive(Copy, Clone, Debug, Error)]
#[cfg_attr(test, derive(PartialEq))]
pub enum QueryError {
    #[error("No results from JSONPath query")]
    NoResults,
    /// Got either 0 or 2+ results for JSON path query
    #[error("Expected exactly one result from query, but got {actual_count}")]
    TooManyResults { actual_count: usize },
}

impl From<ExactlyOneError> for QueryError {
    fn from(error: ExactlyOneError) -> Self {
        match error {
            ExactlyOneError::Empty => Self::NoResults,
            ExactlyOneError::MoreThanOne(n) => {
                Self::TooManyResults { actual_count: n }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::content_type::Json;
    use rstest::rstest;
    use serde_json::json;
    use slumber_util::assert_err;

    /// Test how `query_to_string` handles different types of values returned as
    /// *single results* of a query
    #[rstest]
    #[case::root(
        "$",
        r#"{"array":["hi",1],"bool":true,"int":3,"object":{"a":1,"b":2},"string":"hi!"}"#,
    )]
    #[case::string("$.string", "hi!")]
    #[case::int("$.int", "3")]
    #[case::bool("$.bool", "true")]
    #[case::array("$.array", r#"["hi",1]"#)]
    #[case::object("$.object", r#"{"a":1,"b":2}"#)]
    fn test_query_to_string_types(#[case] query: &str, #[case] expected: &str) {
        let content = json(json!({
            "array": ["hi", 1],
            "bool": true,
            "int": 3,
            "object": {"a": 1, "b": 2},
            "string": "hi!",
        }));
        let query = Query::from_str(query).unwrap();
        let out = query
            .query_to_string(SelectorMode::Single, &*content)
            .unwrap();
        assert_eq!(out, expected);
    }

    /// Test how `query_to_string` handles different query modes
    #[rstest]
    #[case::scalar_single(SelectorMode::Single, "$[0].name", "apple")]
    #[case::scalar_auto(SelectorMode::Auto, "$[0].name", "apple")]
    #[case::multiple_auto(
        SelectorMode::Auto,
        "$[*].name",
        r#"["apple","guava","pear"]"#
    )]
    #[case::none_array(SelectorMode::Array, "$[*].id", "[]")]
    #[case::single_array(SelectorMode::Array, "$[0].name", r#"["apple"]"#)]
    #[case::multiple_array(
        SelectorMode::Array,
        "$[*].name",
        r#"["apple","guava","pear"]"#
    )]
    fn test_query_to_string_modes(
        #[case] mode: SelectorMode,
        #[case] query: &str,
        #[case] expected: &str,
    ) {
        let content = json(json!([
            {"name": "apple"},
            {"name": "guava"},
            {"name": "pear"},
        ]));
        let query = Query::from_str(query).unwrap();
        let out = query.query_to_string(mode, &*content).unwrap();
        assert_eq!(out, expected);
    }

    #[rstest]
    #[case::too_many_results_single(
        SelectorMode::Single,
        "$[*]",
        json(json!([1, 2])),
        "Expected exactly one result from query, but got 2",
    )]
    #[case::no_results_auto(
        SelectorMode::Auto,
        "$[*]",
        json(json!([])),
        "No results from JSONPath query",
    )]
    #[case::no_results_single(
        SelectorMode::Single,
        "$[*]",
        json(json!([])),
        "No results from JSONPath query",
    )]
    fn test_query_to_string_error(
        #[case] mode: SelectorMode,
        #[case] query: &str,
        #[case] content: Box<dyn ResponseContent>,
        #[case] expected_err: &str,
    ) {
        let query = Query::from_str(query).unwrap();
        assert_err!(query.query_to_string(mode, &*content), expected_err);
    }

    /// Helper to create JSON content
    fn json(value: serde_json::Value) -> Box<dyn ResponseContent> {
        Box::new(Json::from(value))
    }
}
