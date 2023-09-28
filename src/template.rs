use crate::{config::Chain, history::RequestHistory, util::ResultExt};
use anyhow::Context;
use derive_more::{Deref, Display, From};
use indexmap::IndexMap;
use regex::Regex;
use serde::Deserialize;
use serde_json_path::{ExactlyOneError, JsonPath};
use std::{
    borrow::Cow,
    env::{self, VarError},
    ops::Deref as _,
    sync::OnceLock,
};
use thiserror::Error;
use tracing::{instrument, trace};

static TEMPLATE_REGEX: OnceLock<Regex> = OnceLock::new();

/// A string that can contain templated content
#[derive(Clone, Debug, Deref, Display, From, Deserialize)]
pub struct TemplateString(String);

/// A little container struct for all the data that the user can access via
/// templating. This is derived from AppState, and will only store references
/// to that state (without cloning).
#[derive(Debug)]
pub struct TemplateContext<'a> {
    /// Technically this could just be an empty IndexMap instead of needing an
    /// option, but that makes it hard when the environment is missing on the
    /// creator's side, because they need to create an empty map and figure out
    /// how to keep it around
    pub environment: Option<&'a IndexMap<String, String>>,
    pub chains: &'a [Chain],
    pub history: &'a RequestHistory,
    /// Additional key=value overrides passed directly from the user
    pub overrides: Option<&'a IndexMap<String, String>>,
}

/// Any error that can occur during template rendering. Generally the generic
/// parameter will be either `&str` (for localized errors) or `String` (for
/// global errors that need to be propagated up).
///
/// The purpose of having a structured error here (while the rest of the app
/// just uses `anyhow`) is to support localized error display in the UI, e.g.
/// showing just one portion of a string in red if that particular template
/// key failed to render.
#[derive(Debug, Error)]
pub enum TemplateError<S: std::fmt::Display> {
    /// Template key could not be parsed
    #[error("Failed to parse template key {key:?}")]
    InvalidKey { key: S },

    /// A basic field key contained an unknown field
    #[error("Unknown field {field:?}")]
    FieldUnknown { field: S },

    #[error("Unknown chain {chain_id:?}")]
    ChainUnknown { chain_id: S },

    /// The chain ID is valid, but the corresponding recipe has no successful
    /// response
    #[error("No response available for chain {chain_id:?}")]
    ChainNoResponse { chain_id: S },

    /// An error occurred while querying with JSON path
    #[error("Error parsing JSON path {path:?} for chain {chain_id:?}")]
    ChainJsonPath {
        chain_id: S,
        path: S,
        #[source]
        error: serde_json_path::ParseError,
    },

    /// Failed to parse the response body as JSON
    #[error("Error parsing response as JSON for chain {chain_id:?}")]
    ChainJsonResponse {
        chain_id: S,
        #[source]
        error: serde_json::Error,
    },

    /// Got either 0 or 2+ results for JSON path query
    #[error("Expected exactly one result for chain {chain_id:?}")]
    ChainInvalidResult {
        chain_id: S,
        #[source]
        error: ExactlyOneError,
    },

    /// Variable either didn't exist or had non-unicode content
    #[error("Error accessing environment variable {variable:?}")]
    EnvironmentVariable {
        variable: S,
        #[source]
        error: VarError,
    },

    /// An error occurred accessing history
    #[error("{0}")]
    History(#[source] anyhow::Error),
}

impl TemplateString {
    /// Render the template string using values from the given context. If an
    /// error occurs, it is returned as general `anyhow` error. If you need a
    /// more specific error, use [Self::render_borrow].
    pub fn render(&self, context: &TemplateContext) -> anyhow::Result<String> {
        self.render_borrow(context)
            .map_err(TemplateError::into_owned)
            .with_context(|| format!("Error rendering template {:?}", self.0))
            .traced()
    }

    /// Render the template string using values from the given state. If an
    /// error occurs, return a borrowed error type that references the template
    /// string. Useful for inline rendering in the UI.
    #[instrument]
    pub fn render_borrow<'a>(
        &'a self,
        context: &'a TemplateContext,
    ) -> Result<String, TemplateError<&'a str>> {
        // Template syntax is simple so it's easiest to just implement it with
        // a regex
        let re = TEMPLATE_REGEX
            .get_or_init(|| Regex::new(r"\{\{\s*([\w\d._-]+)\s*\}\}").unwrap());

        // Regex::replace_all doesn't support fallible replacement, so we
        // have to do it ourselves.
        // https://docs.rs/regex/1.9.5/regex/struct.Regex.html#method.replace_all

        let mut new = String::with_capacity(self.len());
        let mut last_match = 0;
        for captures in re.captures_iter(self) {
            let m = captures.get(0).unwrap();
            new.push_str(&self[last_match..m.start()]);
            let key_raw =
                captures.get(1).expect("Missing key capture group").as_str();
            let key = TemplateKey::parse(key_raw)?;
            let rendered_value = key.into_value().render(context)?;
            trace!(
                key = key_raw,
                value = rendered_value.deref(),
                "Rendered template key"
            );
            // Replace the key with its value
            new.push_str(&rendered_value);
            last_match = m.end();
        }
        new.push_str(&self[last_match..]);

        Ok(new)
    }
}

/// A parsed template key. The variant of this determines how the key will be
/// resolved into a value.
///
/// This also serves as an enumeration of all possible value types. Once a key
/// is parsed, we know its value type and can dynamically dispatch for rendering
/// based on that.
#[derive(Clone, Debug, PartialEq)]
enum TemplateKey<'a> {
    /// A plain field, which can come from the environment or an override
    Field(&'a str),
    /// A value chained from the response of another recipe
    Chain(&'a str),
    /// A value pulled from the process environment
    Environment(&'a str),
}

impl<'a> TemplateKey<'a> {
    /// Parse a string into a key. It'd be nice if this was a `FromStr`
    /// implementation, but that doesn't allow us to attach to the lifetime of
    /// the input `str`.
    fn parse(s: &'a str) -> Result<Self, TemplateError<&'a str>> {
        match s.split('.').collect::<Vec<_>>().as_slice() {
            [key] => Ok(Self::Field(key)),
            ["chains", chain_id] => Ok(Self::Chain(chain_id)),
            ["env", variable] => Ok(Self::Environment(variable)),
            _ => Err(TemplateError::InvalidKey { key: s }),
        }
    }

    /// Convert this key into a renderable value type
    fn into_value(self) -> Box<dyn TemplateSource<'a>> {
        match self {
            TemplateKey::Field(field) => Box::new(FieldSource { field }),
            TemplateKey::Chain(chain_id) => Box::new(ChainSource { chain_id }),
            TemplateKey::Environment(variable) => {
                Box::new(EnvironmentSource { variable })
            }
        }
    }
}

/// A single-type parsed template key, which can be rendered into a string.
/// This should be one implementation of this for each variant of [TemplateKey].
///
/// By breaking `TemplateKey` apart into multiple types, we can split the
/// render logic easily amongst a bunch of functions. It's not technically
/// necessary, just a code organization thing.
trait TemplateSource<'a>: 'a {
    /// Render this intermediate value into a string. Return a Cow because
    /// sometimes this can be a reference to the template context, but
    /// other times it has to be owned data (e.g. when pulling response data
    /// from the history DB).
    fn render(
        &self,
        context: &'a TemplateContext<'a>,
    ) -> Result<Cow<'a, str>, TemplateError<&'a str>>;
}

/// A simple field value (e.g. from the environment or an override)
struct FieldSource<'a> {
    field: &'a str,
}

impl<'a> TemplateSource<'a> for FieldSource<'a> {
    fn render(
        &self,
        context: &'a TemplateContext<'a>,
    ) -> Result<Cow<'a, str>, TemplateError<&'a str>> {
        fn get_opt<'a>(
            map: Option<&'a IndexMap<String, String>>,
            key: &str,
        ) -> Option<&'a String> {
            map?.get(key)
        }

        let field = self.field;
        None
            // Cascade down the the list of maps we want to check
            .or_else(|| get_opt(context.overrides, field))
            .or_else(|| get_opt(context.environment, field))
            .map(Cow::from)
            .ok_or(TemplateError::FieldUnknown { field })
    }
}

/// A chained value from another response
struct ChainSource<'a> {
    chain_id: &'a str,
}

impl<'a> TemplateSource<'a> for ChainSource<'a> {
    fn render(
        &self,
        context: &'a TemplateContext<'a>,
    ) -> Result<Cow<'a, str>, TemplateError<&'a str>> {
        let chain_id = self.chain_id;
        // Resolve chained value
        let chain = context
            .chains
            .iter()
            .find(|chain| chain.id == chain_id)
            .ok_or(TemplateError::ChainUnknown { chain_id })?;
        let response = context
            .history
            .get_last_success(&chain.source)
            .map_err(TemplateError::History)?
            .ok_or(TemplateError::ChainNoResponse { chain_id })?
            .content;

        // Optionally extract a value from the JSON
        match &chain.path {
            Some(path) => {
                // Parse the JSON path
                let path = JsonPath::parse(path).map_err(|err| {
                    TemplateError::ChainJsonPath {
                        chain_id,
                        path,
                        error: err,
                    }
                })?;
                // Parse the response as JSON
                let response_value: serde_json::Value =
                    serde_json::from_str(&response).map_err(|err| {
                        TemplateError::ChainJsonResponse {
                            chain_id,
                            error: err,
                        }
                    })?;
                let found_value = path
                    .query(&response_value)
                    .exactly_one()
                    .map_err(|err| TemplateError::ChainInvalidResult {
                        chain_id,
                        error: err,
                    })?;

                match found_value {
                    serde_json::Value::String(s) => Ok(s.clone().into()),
                    other => Ok(other.to_string().into()),
                }
            }
            None => Ok(response.into()),
        }
    }
}

/// A value sourced from the process's environment
struct EnvironmentSource<'a> {
    variable: &'a str,
}

impl<'a> TemplateSource<'a> for EnvironmentSource<'a> {
    fn render(
        &self,
        _: &'a TemplateContext<'a>,
    ) -> Result<Cow<'a, str>, TemplateError<&'a str>> {
        env::var(self.variable).map(Cow::from).map_err(|err| {
            TemplateError::EnvironmentVariable {
                variable: self.variable,
                error: err,
            }
        })
    }
}

impl<'a> TemplateError<&'a str> {
    /// Convert a borrowed error into an owned one by cloning every string
    pub fn into_owned(self) -> TemplateError<String> {
        match self {
            TemplateError::InvalidKey { key } => TemplateError::InvalidKey {
                key: key.to_owned(),
            },
            TemplateError::FieldUnknown { field } => {
                TemplateError::FieldUnknown {
                    field: field.to_owned(),
                }
            }
            TemplateError::ChainUnknown { chain_id } => {
                TemplateError::ChainUnknown {
                    chain_id: chain_id.to_owned(),
                }
            }
            TemplateError::ChainNoResponse { chain_id } => {
                TemplateError::ChainNoResponse {
                    chain_id: chain_id.to_owned(),
                }
            }
            TemplateError::ChainJsonPath {
                chain_id,
                path,
                error,
            } => TemplateError::ChainJsonPath {
                chain_id: chain_id.to_owned(),
                path: path.to_owned(),
                error,
            },
            TemplateError::ChainJsonResponse { chain_id, error } => {
                TemplateError::ChainJsonResponse {
                    chain_id: chain_id.to_owned(),
                    error,
                }
            }
            TemplateError::ChainInvalidResult { chain_id, error } => {
                TemplateError::ChainInvalidResult {
                    chain_id: chain_id.to_owned(),
                    error,
                }
            }
            TemplateError::EnvironmentVariable { variable, error } => {
                TemplateError::EnvironmentVariable {
                    variable: variable.to_owned(),
                    error,
                }
            }
            TemplateError::History(err) => TemplateError::History(err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::RequestRecipeId,
        factory::*,
        http::{Request, Response},
        util::assert_err,
    };
    use anyhow::anyhow;
    use factori::create;
    use rstest::rstest;
    use serde_json::json;

    /// Test that a field key renders correctly
    #[test]
    fn test_field() {
        let environment = [
            ("user_id".into(), "1".into()),
            ("group_id".into(), "3".into()),
        ]
        .into_iter()
        .collect();
        let overrides = [("user_id".into(), "2".into())].into_iter().collect();
        let context = TemplateContext {
            environment: Some(&environment),
            overrides: Some(&overrides),
            history: &RequestHistory::testing(),
            chains: &[],
        };

        // Success cases
        assert_eq!(
            TemplateString("".into()).render_borrow(&context).unwrap(),
            "".to_owned()
        );
        assert_eq!(
            TemplateString("plain".into())
                .render_borrow(&context)
                .unwrap(),
            "plain".to_owned()
        );
        assert_eq!(
            // Pull from overrides for user_id, env for group_id
            TemplateString("{{user_id}} {{group_id}}".into())
                .render_borrow(&context)
                .unwrap(),
            "2 3".to_owned()
        );

        // Error cases
        assert_err!(
            TemplateString("{{onion_id}}".into()).render_borrow(&context),
            "Unknown field \"onion_id\""
        );
    }

    /// Test success cases with chained responses
    #[rstest]
    #[case(
        None,
        r#"{"array":[1,2],"bool":false,"number":6,"object":{"a":1},"string":"Hello World!"}"#,
    )]
    #[case(Some("$.string"), "Hello World!")]
    #[case(Some("$.number"), "6")]
    #[case(Some("$.bool"), "false")]
    #[case(Some("$.array"), "[1,2]")]
    #[case(Some("$.object"), "{\"a\":1}")]
    fn test_chain(#[case] path: Option<&str>, #[case] expected_value: &str) {
        let recipe_id: RequestRecipeId = "recipe1".into();
        let history = RequestHistory::testing();
        let response_content = json!({
            "string": "Hello World!",
            "number": 6,
            "bool": false,
            "array": [1,2],
            "object": {"a": 1},
        });
        history.add(
            &create!(Request, recipe_id: recipe_id.clone()),
            &Ok(create!(Response, content: response_content.to_string())),
        );
        let context = TemplateContext {
            environment: None,
            overrides: None,
            history: &history,
            chains: &[create!(
                Chain,
                id: "chain1".into(),
                source: recipe_id,
                path: path.map(String::from),
            )],
        };

        assert_eq!(
            TemplateString("{{chains.chain1}}".into())
                .render_borrow(&context)
                .unwrap(),
            expected_value
        );
    }

    /// Test all possible error cases for chained requests. This covers all
    /// chain-specific error variants
    #[rstest]
    #[case(create!(Chain), None, "Unknown chain \"chain1\"")]
    #[case(
        create!(Chain, id: "chain1".into(), source: "recipe1".into()),
        Some((
            create!(Request, recipe_id: "recipe1".into()),
            Err(anyhow!("Bad!")),
        )),
        "No response available for chain \"chain1\"",
    )]
    #[case(
        create!(Chain, id: "chain1".into(), source: "unknown".into()),
        None,
        "No response available for chain \"chain1\"",
    )]
    #[case(
        create!(
            Chain,
            id: "chain1".into(),
            source: "recipe1".into(),
            path: Some("$.".into()),
        ),
        Some((
            create!(Request, recipe_id: "recipe1".into()),
            Ok(create!(Response, content: "{}".into())),
        )),
        "Error parsing JSON path \"$.\" for chain \"chain1\"",
    )]
    #[case(
        create!(
            Chain,
            id: "chain1".into(),
            source: "recipe1".into(),
            path: Some("$.message".into()),
        ),
        Some((
            create!(Request, recipe_id: "recipe1".into()),
            Ok(create!(Response, content: "not json!".into())),
        )),
        "Error parsing response as JSON for chain \"chain1\"",
    )]
    #[case(
        create!(
            Chain,
            id: "chain1".into(),
            source: "recipe1".into(),
            path: Some("$.*".into()),
        ),
        Some((
            create!(Request, recipe_id: "recipe1".into()),
            Ok(create!(Response, content: "[1, 2]".into())),
        )),
        "Expected exactly one result for chain \"chain1\"",
    )]
    fn test_chain_error(
        #[case] chain: Chain,
        // Optional request data to store in history
        #[case] request_response: Option<(Request, anyhow::Result<Response>)>,
        #[case] expected_error: &str,
    ) {
        let history = RequestHistory::testing();
        if let Some((request, response)) = request_response {
            history.add(&request, &response);
        }
        let context = TemplateContext {
            environment: None,
            overrides: None,
            history: &history,
            chains: &[chain],
        };

        assert_err!(
            TemplateString::from("{{chains.chain1}}").render_borrow(&context),
            expected_error
        );
    }

    #[test]
    fn test_environment_success() {
        let context = TemplateContext {
            environment: None,
            overrides: None,
            history: &RequestHistory::testing(),
            chains: &[],
        };

        env::set_var("TEST", "test!");
        assert_eq!(
            TemplateString::from("{{env.TEST}}")
                .render_borrow(&context)
                .unwrap(),
            "test!"
        );
    }

    #[test]
    fn test_environment_error() {
        let context = TemplateContext {
            environment: None,
            overrides: None,
            history: &RequestHistory::testing(),
            chains: &[],
        };

        assert_err!(
            TemplateString::from("{{env.UNKNOWN}}").render_borrow(&context),
            "Error accessing environment variable \"UNKNOWN\""
        );
    }

    /// Test successful parsing *inside* the {{ }}
    #[rstest]
    #[case("field_id", TemplateKey::Field("field_id"))]
    #[case("chains.chain_id", TemplateKey::Chain("chain_id"))]
    // This is "valid", but probably won't match anything
    #[case("chains.", TemplateKey::Chain(""))]
    #[case("env.TEST", TemplateKey::Environment("TEST"))]
    fn test_parse_template_key_success(
        #[case] input: &str,
        #[case] expected_value: TemplateKey,
    ) {
        assert_eq!(TemplateKey::parse(input).unwrap(), expected_value);
    }

    /// Test errors when parsing inside the {{ }}
    #[rstest]
    #[case(".")]
    #[case(".bad")]
    #[case("bad.")]
    #[case("chains.good.bad")]
    fn test_parse_template_key_error(#[case] input: &str) {
        assert_err!(
            TemplateKey::parse(input),
            &format!("Failed to parse template key {input:?}")
        );
    }
}
