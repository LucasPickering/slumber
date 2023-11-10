mod error;
mod prompt;

pub use error::{ChainError, TemplateError, TemplateResult};
pub use prompt::{Prompt, Prompter};

use crate::{
    config::{Chain, ChainSource, RequestRecipeId},
    http::{ContentType, Json, Repository},
    util::ResultExt,
};
use anyhow::Context;
use async_trait::async_trait;
use derive_more::{Deref, From};
use indexmap::IndexMap;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json_path::JsonPath;
use std::{
    env::{self},
    fmt::Debug,
    ops::Deref as _,
    path::Path,
    sync::OnceLock,
};
use tokio::{fs, sync::oneshot};
use tracing::{instrument, trace};

static TEMPLATE_REGEX: OnceLock<Regex> = OnceLock::new();

/// A string that can contain templated content
#[derive(Clone, Debug, Deref, From, PartialEq, Serialize, Deserialize)]
#[deref(forward)]
pub struct TemplateString(String);

/// A little container struct for all the data that the user can access via
/// templating. Unfortunately this has to own all data so templating can be
/// defered into a task.
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
    /// A conduit to ask the user questions
    pub prompter: Box<dyn Prompter>,
}

/// A piece of a rendered template string. A collection of chunks collectively
/// constitutes a rendered string, and those chunks should be contiguous.
#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq))]
pub enum TemplateChunk {
    /// Raw unprocessed text, i.e. something **outside** the `{{ }}`. This is
    /// stored as indexes into the original string, rather than a string
    /// slice, to allow this to be passed between tasks/threads easily. We
    /// could store an owned copy here but that would require copying what
    /// could be a very large block of text.
    Raw { start: usize, end: usize },
    /// Outcome of rendering a template key
    Rendered(String),
    /// An error occurred while rendering a template key
    Error(TemplateError),
}

impl TemplateString {
    /// Render the template string using values from the given context. If an
    /// error occurs, it is returned as general `anyhow` error. If you need a
    /// more specific error, use [Self::render_borrow].
    pub async fn render(
        &self,
        context: &TemplateContext,
        name: &str,
    ) -> anyhow::Result<String> {
        self.render_stitched(context)
            .await
            .with_context(|| format!("Error rendering {name} {:?}", self.0))
            .traced()
    }

    /// Render the template string using values from the given context,
    /// returning the individual rendered chunks. This is useful in any
    /// application where rendered chunks need to be handled differently from
    /// raw chunks, e.g. in render previews.
    #[instrument]
    pub async fn render_chunks(
        &self,
        context: &TemplateContext,
    ) -> Vec<TemplateChunk> {
        // Template syntax is simple so it's easiest to just implement it with
        // a regex
        let re = TEMPLATE_REGEX
            .get_or_init(|| Regex::new(r"\{\{\s*([\w\d._-]+)\s*\}\}").unwrap());

        // Regex::replace_all doesn't support fallible replacement, so we
        // have to do it ourselves.
        // https://docs.rs/regex/1.9.5/regex/struct.Regex.html#method.replace_all

        let mut chunks = Vec::new();
        let mut last_match_end = 0;
        for captures in re.captures_iter(self) {
            let mtch = captures.get(0).unwrap();
            let key_raw =
                captures.get(1).expect("Missing key capture group").as_str();

            // Add the raw string between the last match and this once
            if last_match_end < mtch.start() {
                chunks.push(TemplateChunk::Raw {
                    start: last_match_end,
                    end: mtch.start(),
                });
            }

            // If the key is in the overrides, use the given value without
            // parsing it
            let result = match context.overrides.get(key_raw) {
                Some(value) => {
                    trace!(
                        key = key_raw,
                        value = value,
                        "Rendered template key from override"
                    );
                    Ok(value.into())
                }
                None => {
                    // Standard case - parse the key and render it
                    try {
                        let key = TemplateKey::parse(key_raw)?;
                        let value = key.into_value().render(context).await?;
                        trace!(
                            key = key_raw,
                            value = value.deref(),
                            "Rendered template key"
                        );
                        value
                    }
                }
            };

            // Store the result (success or failure) of rendering
            chunks.push(result.into());
            last_match_end = mtch.end();
        }

        // Add the chunk between the last render and the end
        if last_match_end < self.len() {
            chunks.push(TemplateChunk::Raw {
                start: last_match_end,
                end: self.len(),
            });
        }

        chunks
    }

    /// Helper for stitching chunks together into a single string. If any chunk
    /// failed to render, return an error.
    async fn render_stitched(
        &self,
        context: &TemplateContext,
    ) -> TemplateResult {
        // Render each individual template chunk in the string
        let chunks = self.render_chunks(context).await;

        // Stitch the rendered chunks together into one string
        let mut buffer = String::with_capacity(self.len());
        for chunk in chunks {
            match chunk {
                TemplateChunk::Raw { start, end } => {
                    buffer.push_str(&self.0[start..end]);
                }
                TemplateChunk::Rendered(value) => buffer.push_str(&value),
                TemplateChunk::Error(error) => return Err(error),
            }
        }
        Ok(buffer)
    }
}

impl From<TemplateResult> for TemplateChunk {
    fn from(result: TemplateResult) -> Self {
        match result {
            Ok(value) => Self::Rendered(value),
            Err(error) => Self::Error(error),
        }
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
    const CHAINS_PREFIX: &'static str = "chains";
    const ENVIRONMENT_PREFIX: &'static str = "env";

    /// Parse a string into a key. It'd be nice if this was a `FromStr`
    /// implementation, but that doesn't allow us to attach to the lifetime of
    /// the input `str`.
    fn parse(s: &'a str) -> Result<Self, TemplateError> {
        match s.split('.').collect::<Vec<_>>().as_slice() {
            [key] => Ok(Self::Field(key)),
            [Self::CHAINS_PREFIX, chain_id] => Ok(Self::Chain(chain_id)),
            [Self::ENVIRONMENT_PREFIX, variable] => {
                Ok(Self::Environment(variable))
            }
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
    async fn render(&self, context: &'a TemplateContext) -> TemplateResult;
}

/// A simple field value (e.g. from the profile or an override)
struct FieldTemplateSource<'a> {
    field: &'a str,
}

#[async_trait]
impl<'a> TemplateSource<'a> for FieldTemplateSource<'a> {
    async fn render(&self, context: &'a TemplateContext) -> TemplateResult {
        let field = self.field;
        context.profile.get(field).cloned().ok_or_else(|| {
            TemplateError::FieldUnknown {
                field: field.to_owned(),
            }
        })
    }
}

/// A chained value from a complex source. Could be an HTTP response, file, etc.
struct ChainTemplateSource<'a> {
    chain_id: &'a str,
}

#[async_trait]
impl<'a> TemplateSource<'a> for ChainTemplateSource<'a> {
    async fn render(&self, context: &'a TemplateContext) -> TemplateResult {
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
                ChainSource::Prompt(label) => {
                    self.render_prompt(
                        context,
                        label.as_deref(),
                        chain.sensitive,
                    )
                    .await?
                }
            };

            // If a selector path is present, filter down the value
            match &chain.selector {
                Some(path) => self.apply_selector(&value, path)?,
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
    ) -> Result<String, ChainError> {
        let record = context
            .repository
            .get_last(recipe_id)
            .await
            .map_err(ChainError::Repository)?
            .ok_or(ChainError::NoResponse)?;

        Ok(record.response.body.into_text())
    }

    /// Render a chained value from a file
    async fn render_file(&self, path: &'a Path) -> Result<String, ChainError> {
        fs::read_to_string(path)
            .await
            .map_err(|err| ChainError::File {
                path: path.to_owned(),
                error: err,
            })
    }

    /// Render a value by asking the user to provide it
    async fn render_prompt(
        &self,
        context: &'a TemplateContext,
        label: Option<&str>,
        sensitive: bool,
    ) -> Result<String, ChainError> {
        // Use the prompter to ask the user a question, and wait for a response
        // on the prompt channel
        let (tx, rx) = oneshot::channel();
        context.prompter.prompt(Prompt {
            label: label.unwrap_or(self.chain_id).into(),
            sensitive,
            channel: tx,
        });
        rx.await.map_err(|_| ChainError::PromptNoResponse)
    }

    /// Apply a selector path to a string value to filter it down. Right now
    /// this only supports JSONpath but we could add support for more in the
    /// future. The string value will be parsed as a JSON value.
    fn apply_selector(
        &self,
        value: &str,
        selector: &JsonPath,
    ) -> Result<String, ChainError> {
        // Parse the response as JSON. Intentionally ignore the
        // content-type. If the user wants to treat it as JSON, we
        // should allow that even if the server is wrong.
        let json_value = Json::parse(value)
            .map_err(|err| ChainError::ParseResponse { error: err })?;

        // Apply the path to the json
        let found_value = selector
            .query(&json_value)
            .exactly_one()
            .map_err(|err| ChainError::InvalidResult { error: err })?;

        match found_value {
            serde_json::Value::String(s) => Ok(s.clone()),
            other => Ok(other.to_string()),
        }
    }
}

/// A value sourced from the process's environment
struct EnvironmentTemplateSource<'a> {
    variable: &'a str,
}

#[async_trait]
impl<'a> TemplateSource<'a> for EnvironmentTemplateSource<'a> {
    async fn render(&self, _: &'a TemplateContext) -> TemplateResult {
        env::var(self.variable).map_err(|err| {
            TemplateError::EnvironmentVariable {
                variable: self.variable.to_owned(),
                error: err,
            }
        })
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
    use factori::create;
    use indexmap::indexmap;
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
        assert_eq!(
            // Test complex stitching. Emoji is important to test because the
            // stitching uses character indexes
            render!("start {{user_id}} 🧡💛 {{group_id}} end", context)
                .unwrap(),
            "start 2 🧡💛 3 end".to_owned()
        );

        // Error cases
        assert_err!(
            render!("{{onion_id}}", context),
            "Unknown field \"onion_id\"",
            true
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
        let response =
            create!(Response, body: response_body.to_string().into());
        repository
            .insert_test(
                &create!(RequestRecord, request: request, response: response),
            )
            .await
            .unwrap();
        let selector = selector.map(|s| s.parse().unwrap());
        let chains = vec![create!(
            Chain,
            id: "chain1".into(),
            source: ChainSource::Request(recipe_id),
            selector: selector,
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
    #[case(create!(Chain), None, "Unknown chain")]
    #[case(
        create!(Chain, id: "chain1".into(), source: ChainSource::Request("unknown".into())),
        None,
        "No response available",
    )]
    #[case(
        create!(
            Chain,
            id: "chain1".into(),
            source: ChainSource::Request("recipe1".into()),
            selector: Some("$.message".parse().unwrap()),
        ),
        Some((
            create!(Request, recipe_id: "recipe1".into()),
            create!(Response, body: "not json!".into()),
        )),
        "Error parsing response",
    )]
    #[case(
        create!(
            Chain,
            id: "chain1".into(),
            source: ChainSource::Request("recipe1".into()),
            selector: Some("$.*".parse().unwrap()),
        ),
        Some((
            create!(Request, recipe_id: "recipe1".into()),
            create!(Response, body: "[1, 2]".into()),
        )),
        "Expected exactly one result",
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

        assert_err!(
            render!("{{chains.chain1}}", context),
            expected_error,
            true
        );
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
        )];
        let context = create!(TemplateContext, chains: chains);

        assert_err!(
            render!("{{chains.chain1}}", context),
            "Error reading from file",
            true
        );
    }

    #[tokio::test]
    async fn test_chain_prompt() {
        let chains = vec![create!(
            Chain,
            id: "chain1".into(),
            source: ChainSource::Prompt(Some("password".into())),
        )];
        let context = create!(
            TemplateContext,
            chains: chains,
            // Prompter gives no response
            prompter: Box::new(TestPrompter::new(Some("hello!"))),
        );

        assert_eq!(render!("{{chains.chain1}}", context).unwrap(), "hello!");
    }

    /// Prompting gone wrong
    #[tokio::test]
    async fn test_chain_prompt_error() {
        let chains = vec![create!(
            Chain,
            id: "chain1".into(),
            source: ChainSource::Prompt(Some("password".into())),
        )];
        let context = create!(
            TemplateContext,
            chains: chains,
            // Prompter gives no response
            prompter: Box::new(TestPrompter::new::<String>(None)),
        );

        assert_err!(
            render!("{{chains.chain1}}", context),
            "No response from prompt",
            true
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
            "Error accessing environment variable \"UNKNOWN\"",
            true
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
            &format!("Failed to parse template key {input:?}"),
            true
        );
    }

    /// Test rendering into individual chunks
    #[tokio::test]
    async fn test_render_chunks() {
        let profile = indexmap! {
            "user_id".into() => "🧡💛".into()
        };
        let context = create!(
            TemplateContext,
            profile: profile,
        );

        let chunks =
            TemplateString("intro {{user_id}} 💚💙💜 {{unknown}} outro".into())
                .render_chunks(&context)
                .await;
        assert_eq!(
            chunks,
            vec![
                TemplateChunk::Raw { start: 0, end: 6 },
                TemplateChunk::Rendered("🧡💛".into()),
                // Each emoji is 4 bytes
                TemplateChunk::Raw { start: 17, end: 31 },
                TemplateChunk::Error(TemplateError::FieldUnknown {
                    field: "unknown".into()
                }),
                TemplateChunk::Raw { start: 42, end: 48 },
            ]
        );
    }

    /// Helper for rendering a string
    macro_rules! render {
        ($template:expr, $context:expr) => {
            TemplateString($template.into())
                .render_stitched(&$context)
                .await
        };
    }

    use render;
}
