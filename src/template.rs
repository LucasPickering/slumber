use crate::{
    config::{Chain, ChainSource, RequestRecipeId},
    http::{ContentType, Json, Repository},
    util::ResultExt,
};
use anyhow::Context;
use async_trait::async_trait;
use derive_more::{Deref, Display, From};
use indexmap::IndexMap;
use regex::Regex;
use serde::Deserialize;
use serde_json_path::{ExactlyOneError, JsonPath};
use std::{
    borrow::Cow,
    env::{self, VarError},
    io,
    ops::Deref as _,
    path::{Path, PathBuf},
    sync::OnceLock,
};
use thiserror::Error;
use tokio::fs;
use tracing::{instrument, trace};

static TEMPLATE_REGEX: OnceLock<Regex> = OnceLock::new();

/// A string that can contain templated content
#[derive(Clone, Debug, Deref, Display, From, Deserialize)]
pub struct TemplateString(String);

/// A little container struct for all the data that the user can access via
/// templating. This is derived from AppState, and will only store references
/// to that state (without cloning).
#[derive(Debug)]
pub struct TemplateContext {
    /// Key-value mapping
    pub profile: IndexMap<String, String>,
    /// Chained values from dynamic sources
    pub chains: Vec<Chain>,
    /// Needed for accessing response bodies for chaining
    pub repository: Repository,
    /// Additional key=value overrides passed directly from the user
    pub overrides: IndexMap<String, String>,
}

type TemplateResult<'a> = Result<Cow<'a, str>, TemplateError>;

impl TemplateString {
    /// Render the template string using values from the given context. If an
    /// error occurs, it is returned as general `anyhow` error. If you need a
    /// more specific error, use [Self::render_borrow].
    pub async fn render(
        &self,
        context: &TemplateContext,
    ) -> anyhow::Result<String> {
        self.render_borrow(context)
            .await
            .with_context(|| format!("Error rendering template {:?}", self.0))
            .traced()
    }

    /// Render the template string using values from the given context. Useful
    /// for inline rendering in the UI.
    #[instrument]
    pub async fn render_borrow<'a>(
        &'a self,
        context: &'a TemplateContext,
    ) -> Result<String, TemplateError> {
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
            let rendered_value = key.into_value().render(context).await?;
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
    /// A plain field, which can come from the profile or an override
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
    fn parse(s: &'a str) -> Result<Self, TemplateError> {
        match s.split('.').collect::<Vec<_>>().as_slice() {
            [key] => Ok(Self::Field(key)),
            ["chains", chain_id] => Ok(Self::Chain(chain_id)),
            ["env", variable] => Ok(Self::Environment(variable)),
            _ => Err(TemplateError::InvalidKey { key: s.to_owned() }),
        }
    }

    /// Convert this key into a renderable value type
    fn into_value(self) -> Box<dyn TemplateSource<'a>> {
        match self {
            TemplateKey::Field(field) => {
                Box::new(FieldTemplateSource { field })
            }
            TemplateKey::Chain(chain_id) => {
                Box::new(ChainTemplateSource { chain_id })
            }
            TemplateKey::Environment(variable) => {
                Box::new(EnvironmentTemplateSource { variable })
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
#[async_trait]
trait TemplateSource<'a>: 'a + Send + Sync {
    /// Render this intermediate value into a string. Return a Cow because
    /// sometimes this can be a reference to the template context, but
    /// other times it has to be owned data (e.g. when pulling response data
    /// from the repository).
    async fn render(&self, context: &'a TemplateContext) -> TemplateResult<'a>;
}

/// A simple field value (e.g. from the profile or an override)
struct FieldTemplateSource<'a> {
    field: &'a str,
}

#[async_trait]
impl<'a> TemplateSource<'a> for FieldTemplateSource<'a> {
    async fn render(&self, context: &'a TemplateContext) -> TemplateResult<'a> {
        let field = self.field;
        None
            // Cascade down the the list of maps we want to check
            .or_else(|| context.overrides.get(field))
            .or_else(|| context.profile.get(field))
            .map(Cow::from)
            .ok_or(TemplateError::FieldUnknown {
                field: field.to_owned(),
            })
    }
}

/// A chained value from a complex source. Could be an HTTP response, file, etc.
struct ChainTemplateSource<'a> {
    chain_id: &'a str,
}

#[async_trait]
impl<'a> TemplateSource<'a> for ChainTemplateSource<'a> {
    async fn render(&self, context: &'a TemplateContext) -> TemplateResult<'a> {
        let chain_id = self.chain_id;

        // Any error in here is the chain error subtype
        let result: Result<_, ChainError> = try {
            // Resolve chained value
            let chain = context
                .chains
                .iter()
                .find(|chain| chain.id == chain_id)
                .ok_or(ChainError::Unknown)?;

            // Resolve the value based on the source type
            let value = match &chain.source {
                ChainSource::Request(recipe_id) => {
                    self.render_request(context, recipe_id).await?
                }
                ChainSource::File(path) => self.render_file(path).await?,
            };

            // If a selector path is present, filter down the value
            match &chain.selector {
                Some(path) => self.apply_selector(value, path)?,
                None => value,
            }
        };

        // Wrap the chain error into a TemplateError
        result.map_err(|error| TemplateError::Chain {
            chain_id: chain_id.to_owned(),
            error,
        })
    }
}

impl<'a> ChainTemplateSource<'a> {
    /// Render a chained template value from a response
    async fn render_request(
        &self,
        context: &'a TemplateContext,
        recipe_id: &RequestRecipeId,
    ) -> Result<Cow<'a, str>, ChainError> {
        let record = context
            .repository
            .get_last(recipe_id)
            .await
            .map_err(ChainError::Repository)?
            .ok_or(ChainError::NoResponse)?;

        Ok(record.response.body.into())
    }

    /// Render a chained value from a file
    async fn render_file(
        &self,
        path: &'a Path,
    ) -> Result<Cow<'a, str>, ChainError> {
        fs::read_to_string(path)
            .await
            .map(Cow::from)
            .map_err(|err| ChainError::File {
                path: path.to_owned(),
                error: err,
            })
    }

    /// Apply a selector path to a string value to filter it down. Right now
    /// this only supports JSONpath but we could add support for more in the
    /// future. The string value will be parsed as a JSON value.
    fn apply_selector(
        &self,
        value: Cow<'_, str>,
        selector: &'a str,
    ) -> Result<Cow<'a, str>, ChainError> {
        // Parse the JSON path
        let path =
            JsonPath::parse(selector).map_err(|err| ChainError::JsonPath {
                selector: selector.to_owned(),
                error: err,
            })?;

        // Parse the response as JSON. Intentionally ignore the
        // content-type. If the user wants to treat it as JSON, we
        // should allow that even if the server is wrong.
        let json_value = Json::parse(&value)
            .map_err(|err| ChainError::ParseResponse { error: err })?;

        // Apply the path to the json
        let found_value = path
            .query(&json_value)
            .exactly_one()
            .map_err(|err| ChainError::InvalidResult { error: err })?;

        match found_value {
            serde_json::Value::String(s) => Ok(s.clone().into()),
            other => Ok(other.to_string().into()),
        }
    }
}

/// A value sourced from the process's environment
struct EnvironmentTemplateSource<'a> {
    variable: &'a str,
}

#[async_trait]
impl<'a> TemplateSource<'a> for EnvironmentTemplateSource<'a> {
    async fn render(&self, _: &'a TemplateContext) -> TemplateResult<'a> {
        env::var(self.variable).map(Cow::from).map_err(|err| {
            TemplateError::EnvironmentVariable {
                variable: self.variable.to_owned(),
                error: err,
            }
        })
    }
}

/// Any error that can occur during template rendering. The purpose of having a
/// structured error here (while the rest of the app just uses `anyhow`) is to
/// support localized error display in the UI, e.g. showing just one portion of
/// a string in red if that particular template key failed to render.
///
/// The error always holds owned data so it can be detached from the lifetime
/// of the template context. This requires a mild amount of cloning in error
/// cases, but those should be infrequent so it's fine.
#[derive(Debug, Error)]
pub enum TemplateError {
    /// Template key could not be parsed
    #[error("Failed to parse template key {key:?}")]
    InvalidKey { key: String },

    /// A basic field key contained an unknown field
    #[error("Unknown field {field:?}")]
    FieldUnknown { field: String },

    #[error("Error resolving chain {chain_id:?}")]
    Chain {
        chain_id: String,
        #[source]
        error: ChainError,
    },

    /// Variable either didn't exist or had non-unicode content
    #[error("Error accessing environment variable {variable:?}")]
    EnvironmentVariable {
        variable: String,
        #[source]
        error: VarError,
    },
}

/// An error sub-type, for any error that occurs while resolving a chained
/// value. This is factored out because they all need to be paired with a chain
/// ID.
#[derive(Debug, Error)]
pub enum ChainError {
    /// Reference to a chain that doesn't exist
    #[error("Unknown chain")]
    Unknown,
    /// An error occurred accessing the request repository. This error is
    /// generated by our code so we don't need any extra context.
    #[error("{0}")]
    Repository(#[source] anyhow::Error),
    /// The chain ID is valid, but the corresponding recipe has no successful
    /// response
    #[error("No response available")]
    NoResponse,
    #[error("Error parsing JSON path {selector:?}")]
    JsonPath {
        selector: String,
        #[source]
        error: serde_json_path::ParseError,
    },
    /// Failed to parse the response body before applying a selector
    #[error("Error parsing response")]
    ParseResponse {
        #[source]
        error: anyhow::Error,
    },
    /// Got either 0 or 2+ results for JSON path query
    #[error("Expected exactly one result from selector")]
    InvalidResult {
        #[source]
        error: ExactlyOneError,
    },
    #[error("Error reading from file {path:?}")]
    File {
        path: PathBuf,
        #[source]
        error: io::Error,
    },
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
    use factori::create;
    use rstest::rstest;
    use serde_json::json;

    /// Test that a field key renders correctly
    #[tokio::test]
    async fn test_field() {
        let profile = [
            ("user_id".into(), "1".into()),
            ("group_id".into(), "3".into()),
        ]
        .into_iter()
        .collect();
        let overrides = [("user_id".into(), "2".into())].into_iter().collect();
        let context = create!(
            TemplateContext,
            profile: profile,
            overrides: overrides,
        );

        // Success cases
        assert_eq!(render!("", context).unwrap(), "".to_owned());
        assert_eq!(render!("plain", context).unwrap(), "plain".to_owned());
        assert_eq!(
            // Pull from overrides for user_id, profile for group_id
            render!("{{user_id}} {{group_id}}", context).unwrap(),
            "2 3".to_owned()
        );

        // Error cases
        assert_err!(
            render!("{{onion_id}}", context),
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
    #[tokio::test]
    async fn test_chain_request(
        #[case] selector: Option<&str>,
        #[case] expected_value: &str,
    ) {
        let recipe_id: RequestRecipeId = "recipe1".into();
        let repository = Repository::testing();
        let response_body = json!({
            "string": "Hello World!",
            "number": 6,
            "bool": false,
            "array": [1,2],
            "object": {"a": 1},
        });
        let request = create!(Request, recipe_id: recipe_id.clone());
        let response = create!(Response, body: response_body.to_string());
        repository
            .insert_test(
                &create!(RequestRecord, request: request, response: response),
            )
            .await
            .unwrap();
        let chains = vec![create!(
            Chain,
            id: "chain1".into(),
            source: ChainSource::Request(recipe_id),
            selector: selector.map(String::from),
        )];
        let context = create!(
            TemplateContext, repository: repository, chains: chains,
        );

        assert_eq!(
            render!("{{chains.chain1}}", context).unwrap(),
            expected_value
        );
    }

    /// Test all possible error cases for chained requests. This covers all
    /// chain-specific error variants
    #[rstest]
    #[case(create!(Chain), None, "Unknown chain \"chain1\"")]
    #[case(
        create!(Chain, id: "chain1".into(), source: ChainSource::Request("unknown".into())),
        None,
        "No response available for chain \"chain1\"",
    )]
    #[case(
        create!(
            Chain,
            id: "chain1".into(),
            source: ChainSource::Request("recipe1".into()),
            selector: Some("$.".into()),
        ),
        Some((
            create!(Request, recipe_id: "recipe1".into()),
            create!(Response, body: "{}".into()),
        )),
        "Error parsing JSON path \"$.\" for chain \"chain1\"",
    )]
    #[case(
        create!(
            Chain,
            id: "chain1".into(),
            source: ChainSource::Request("recipe1".into()),
            selector: Some("$.message".into()),
        ),
        Some((
            create!(Request, recipe_id: "recipe1".into()),
            create!(Response, body: "not json!".into()),
        )),
        "Error parsing response for chain \"chain1\"",
    )]
    #[case(
        create!(
            Chain,
            id: "chain1".into(),
            source: ChainSource::Request("recipe1".into()),
            selector: Some("$.*".into()),
        ),
        Some((
            create!(Request, recipe_id: "recipe1".into()),
            create!(Response, body: "[1, 2]".into()),
        )),
        "Expected exactly one result for chain \"chain1\"",
    )]
    #[tokio::test]
    async fn test_chain_request_error(
        #[case] chain: Chain,
        // Optional request data to store in the repository
        #[case] request_response: Option<(Request, Response)>,
        #[case] expected_error: &str,
    ) {
        let repository = Repository::testing();
        if let Some((request, response)) = request_response {
            repository
                .insert_test(&create!(
                RequestRecord, request: request, response: response))
                .await
                .unwrap();
        }
        let chains = vec![chain];
        let context = create!(
            TemplateContext, repository: repository, chains: chains
        );

        assert_err!(render!("{{chains.chain1}}", context), expected_error);
    }

    /// Test success with chained file
    #[tokio::test]
    async fn test_chain_file() {
        // Create a temp file that we'll read from
        let temp_dir = env::temp_dir();
        let file_path = temp_dir.join("stuff.txt");
        fs::write(&file_path, "hello!").await.unwrap();

        let chains = vec![create!(
            Chain,
            id: "chain1".into(),
            source: ChainSource::File(file_path),
            selector: None,
        )];
        let context = create!(TemplateContext, chains: chains);

        assert_eq!(render!("{{chains.chain1}}", context).unwrap(), "hello!");
    }

    /// Test failure with chained file
    #[tokio::test]
    async fn test_chain_file_error() {
        let chains = vec![create!(
            Chain,
            id: "chain1".into(),
            source: ChainSource::File("not-a-real-file".into()),
            selector: None,
        )];
        let context = create!(TemplateContext, chains: chains);

        assert_err!(
            render!("{{chains.chain1}}", context),
            "Error reading from file"
        );
    }

    #[tokio::test]
    async fn test_environment_success() {
        let context = create!(TemplateContext);
        env::set_var("TEST", "test!");
        assert_eq!(render!("{{env.TEST}}", context).unwrap(), "test!");
    }

    #[tokio::test]
    async fn test_environment_error() {
        let context = create!(TemplateContext);
        assert_err!(
            render!("{{env.UNKNOWN}}", context),
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

    /// Helper for rendering a string
    macro_rules! render {
        ($template:expr, $context:expr) => {
            TemplateString($template.into())
                .render_borrow(&$context)
                .await
        };
    }
    use render;
}
