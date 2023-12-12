mod error;
mod parse;
mod prompt;
mod render;

pub use error::{ChainError, TemplateError};
pub use parse::Span;
pub use prompt::{Prompt, Prompter};

use crate::{
    collection::{Chain, ChainId, ProfileValue},
    db::CollectionDatabase,
    template::{
        error::TemplateParseError,
        parse::{TemplateInputChunk, CHAIN_PREFIX, ENV_PREFIX},
    },
};
use derive_more::{Deref, Display};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::fmt::Debug;

/// A little container struct for all the data that the user can access via
/// templating. Unfortunately this has to own all data so templating can be
/// defered into a task.
#[derive(Debug)]
pub struct TemplateContext {
    /// Key-value mapping
    pub profile: IndexMap<String, ProfileValue>,
    /// Chained values from dynamic sources
    pub chains: IndexMap<ChainId, Chain>,
    /// Needed for accessing response bodies for chaining
    pub database: CollectionDatabase,
    /// Additional key=value overrides passed directly from the user
    pub overrides: IndexMap<String, String>,
    /// A conduit to ask the user questions
    pub prompter: Box<dyn Prompter>,
}

/// A immutable string that can contain templated content. The string is parsed
/// during creation to identify template keys, hence the immutability.
#[derive(Clone, Debug, Deref, Display, Serialize, Deserialize)]
#[cfg_attr(test, derive(PartialEq))]
#[display("{template}")]
#[serde(try_from = "String", into = "String")]
pub struct Template {
    #[deref(forward)]
    template: String,
    /// Pre-parsed chunks of the template. We can't store slices here because
    /// that would be self-referential, so just store locations. These chunks
    /// are contiguous and span the whole template.
    chunks: Vec<TemplateInputChunk<Span>>,
}

impl Template {
    /// Get a substring of this template. Panics if the span is out of range
    pub fn substring(&self, span: Span) -> &str {
        &self.template[span.start()..span.end()]
    }

    /// Create a new template **without parsing**. The created template should
    /// *never* be rendered. This is only useful when creating templates purely
    /// for the purpose of being serialized, e.g. when importing an external
    /// config into a request collection.
    ///
    /// If you try to render this thing, you'll get garbage. The "correct" thing
    /// to do would be to add some safeguards to make that impossible (either
    /// type state or a runtime check), but it's not worth the extra code for
    /// something that is very unlikely to happen. It says "dangerous", don't be
    /// stupid.
    pub(crate) fn dangerous_new(template: String) -> Self {
        Self {
            template,
            chunks: Vec::new(),
        }
    }
}

impl TryFrom<String> for Template {
    type Error = TemplateParseError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl From<Template> for String {
    fn from(value: Template) -> Self {
        value.template
    }
}

/// A piece of a rendered template string. A collection of chunks collectively
/// constitutes a rendered string, and those chunks should be contiguous.
#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq))]
pub enum TemplateChunk {
    /// Raw unprocessed text, i.e. something **outside** the `{{ }}`. This is
    /// stored as a span in the original string, rather than a string slice, to
    /// allow this to be passed between tasks/threads easily. We could store an
    /// owned copy here but that would require copying what could be a very
    /// large block of text.
    Raw(Span),
    /// Outcome of rendering a template key
    Rendered { value: String, sensitive: bool },
    /// An error occurred while rendering a template key
    Error(TemplateError),
}

/// A parsed template key. The variant of this determines how the key will be
/// resolved into a value.
///
/// This also serves as an enumeration of all possible value types. Once a key
/// is parsed, we know its value type and can dynamically dispatch for rendering
/// based on that.
///
/// The generic parameter defines *how* the key data is stored. Ideally we could
/// just store a `&str`, but that isn't possible when this is part of a
/// `Template`, because it would create a self-referential pointer. In that
/// case, we can store a `Span` which points back to its source in the template.
///
/// The `Display` impl here should return exactly what this was parsed from.
/// This is important for matching override keys during rendering.
#[derive(Copy, Clone, Debug, Display)]
#[cfg_attr(test, derive(PartialEq))]
enum TemplateKey<T> {
    /// A plain field, which can come from the profile or an override
    Field(T),
    /// A value from a predefined chain of another recipe
    #[display("{CHAIN_PREFIX}{_0}")]
    Chain(T),
    /// A value pulled from the process environment
    #[display("{ENV_PREFIX}{_0}")]
    Environment(T),
}

impl<T> TemplateKey<T> {
    /// Map the internal data using the given function. Useful for mapping
    /// string slices to spans and vice versa.
    fn map<U>(self, f: impl Fn(T) -> U) -> TemplateKey<U> {
        match self {
            Self::Field(value) => TemplateKey::Field(f(value)),
            Self::Chain(value) => TemplateKey::Chain(f(value)),
            Self::Environment(value) => TemplateKey::Environment(f(value)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        collection::{ChainSource, RequestRecipeId},
        factory::*,
        http::{Request, Response},
        util::assert_err,
    };
    use factori::create;
    use indexmap::indexmap;
    use rstest::rstest;
    use serde_json::json;
    use std::env;
    use tokio::fs;

    /// Test overriding all key types, as well as missing keys
    #[tokio::test]
    async fn test_override() {
        let profile = indexmap! {"field1".into() => "field".into()};
        let source = ChainSource::Command(
            ["echo", "chain"]
                .iter()
                .cloned()
                .map(String::from)
                .collect(),
        );
        let overrides = indexmap! {
            "field1".into() => "override".into(),
            "chains.chain1".into() => "override".into(),
            "env.ENV1".into() => "override".into(),
            "override1".into() => "override".into(),
        };
        let chains =
            indexmap! {"chain1".into() => create!(Chain, source: source)};
        let context = create!(
            TemplateContext,
            profile: profile,
            chains: chains,
            overrides: overrides,
        );

        assert_eq!(
            render!("{{field1}}", context).unwrap(),
            "override".to_owned()
        );
        assert_eq!(
            render!("{{chains.chain1}}", context).unwrap(),
            "override".to_owned()
        );
        assert_eq!(
            render!("{{env.ENV1}}", context).unwrap(),
            "override".to_owned()
        );
        assert_eq!(
            render!("{{override1}}", context).unwrap(),
            "override".to_owned()
        );
    }

    /// Test that a field key renders correctly
    #[tokio::test]
    async fn test_field() {
        let nested_template =
            Template::parse("user id: {{user_id}}".into()).unwrap();
        let profile = indexmap! {
            "user_id".into() => "1".into(),
            "group_id".into() => "3".into(),
            "recursive".into() => ProfileValue::Template(nested_template),
        };
        let context = create!(TemplateContext, profile: profile);

        assert_eq!(&render!("", context).unwrap(), "");
        assert_eq!(&render!("plain", context).unwrap(), "plain");
        assert_eq!(&render!("{{recursive}}", context).unwrap(), "user id: 1");
        assert_eq!(
            // Test complex stitching. Emoji is important to test because the
            // stitching uses character indexes
            &render!("start {{user_id}} 🧡💛 {{group_id}} end", context)
                .unwrap(),
            "start 1 🧡💛 3 end"
        );
    }

    /// Potential error cases for a profile field
    #[tokio::test]
    async fn test_field_error() {
        let nested_template = Template::parse("{{onion_id}}".into()).unwrap();
        let profile = indexmap! {
            "recursive".into() => ProfileValue::Template(nested_template),
        };
        let context = create!(TemplateContext, profile: profile);

        assert_err!(
            render!("{{onion_id}}", context),
            "Unknown field `onion_id`"
        );
        assert_err!(
            render!("{{recursive}}", context),
            "Error in nested template `{{onion_id}}`: Unknown field `onion_id`"
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
        let database = CollectionDatabase::testing();
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
        database
            .insert_request(
                &create!(RequestRecord, request: request, response: response),
            )
            .unwrap();
        let selector = selector.map(|s| s.parse().unwrap());
        let chains = indexmap! {"chain1".into() => create!(
            Chain,
            source: ChainSource::Request(recipe_id),
            selector: selector,
        )};
        let context = create!(
            TemplateContext, database: database, chains: chains,
        );

        assert_eq!(
            render!("{{chains.chain1}}", context).unwrap(),
            expected_value
        );
    }

    /// Test all possible error cases for chained requests. This covers all
    /// chain-specific error variants
    #[rstest]
    #[case("unknown", create!(Chain), None, "Unknown chain")]
    #[case(
        "chain1",
        create!(Chain, source: ChainSource::Request("unknown".into())),
        None,
        "No response available",
    )]
    #[case(
        "chain1",
        create!(
            Chain,
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
        "chain1",
        create!(
            Chain,
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
        #[case] chain_id: impl Into<ChainId>,
        #[case] chain: Chain,
        // Optional request data to store in the database
        #[case] request_response: Option<(Request, Response)>,
        #[case] expected_error: &str,
    ) {
        let database = CollectionDatabase::testing();
        if let Some((request, response)) = request_response {
            database
                .insert_request(&create!(
                RequestRecord, request: request, response: response))
                .unwrap();
        }
        let chains = indexmap! {chain_id.into() => chain};
        let context = create!(
            TemplateContext, database: database, chains: chains
        );

        assert_err!(render!("{{chains.chain1}}", context), expected_error);
    }

    /// Test success with chained command
    #[tokio::test]
    async fn test_chain_command() {
        let command = vec!["echo".into(), "-n".into(), "hello!".into()];
        let chains = indexmap! {"chain1".into() => create!(
            Chain,
            source: ChainSource::Command(command),
        )};
        let context = create!(TemplateContext, chains: chains);

        assert_eq!(render!("{{chains.chain1}}", context).unwrap(), "hello!");
    }

    /// Test failure with chained command
    #[rstest]
    #[case(&[], "No command given")]
    #[case(&["totally not a program"], "No such file or directory")]
    #[case(&["head", "/dev/random"], "invalid utf-8 sequence")]
    #[tokio::test]
    async fn test_chain_command_error(
        #[case] command: &[&str],
        #[case] expected_error: &str,
    ) {
        let source = ChainSource::Command(
            command.iter().cloned().map(String::from).collect(),
        );
        let chains =
            indexmap! {"chain1".into() => create!(Chain, source: source)};
        let context = create!(TemplateContext, chains: chains);

        assert_err!(render!("{{chains.chain1}}", context), expected_error);
    }

    /// Test success with chained file
    #[tokio::test]
    async fn test_chain_file() {
        // Create a temp file that we'll read from
        let temp_dir = env::temp_dir();
        let file_path = temp_dir.join("stuff.txt");
        fs::write(&file_path, "hello!").await.unwrap();

        let chains = indexmap! {"chain1".into() => create!(
            Chain,
            source: ChainSource::File(file_path),
        )};
        let context = create!(TemplateContext, chains: chains);

        assert_eq!(render!("{{chains.chain1}}", context).unwrap(), "hello!");
    }

    /// Test failure with chained file
    #[tokio::test]
    async fn test_chain_file_error() {
        let chains = indexmap! {"chain1".into() => create!(
            Chain,
            source: ChainSource::File("not-a-real-file".into()),
        )};
        let context = create!(TemplateContext, chains: chains);

        assert_err!(
            render!("{{chains.chain1}}", context),
            "Error reading from file"
        );
    }

    #[tokio::test]
    async fn test_chain_prompt() {
        let chains = indexmap! {"chain1".into() => create!(
            Chain,
            source: ChainSource::Prompt(Some("password".into())),
        )};
        let context = create!(
            TemplateContext,
            chains: chains,
            prompter: Box::new(TestPrompter::new(Some("hello!"))),
        );

        assert_eq!(render!("{{chains.chain1}}", context).unwrap(), "hello!");
    }

    /// Prompting gone wrong
    #[tokio::test]
    async fn test_chain_prompt_error() {
        let chains = indexmap! {"chain1".into() => create!(
            Chain,
            source: ChainSource::Prompt(Some("password".into())),
        )};
        let context = create!(
            TemplateContext,
            chains: chains,
            // Prompter gives no response
            prompter: Box::new(TestPrompter::new::<String>(None)),
        );

        assert_err!(
            render!("{{chains.chain1}}", context),
            "No response from prompt"
        );
    }

    /// Values marked sensitive should have that flag set in the rendered output
    #[tokio::test]
    async fn test_chain_sensitive() {
        let chains = indexmap! {"chain1".into() => create!(
            Chain,
            source: ChainSource::Prompt(Some("password".into())),
            sensitive: true,
        )};
        let context = create!(
            TemplateContext,
            chains: chains,
            // Prompter gives no response
            prompter: Box::new(TestPrompter::new(Some("hello!"))),
        );
        assert_eq!(
            Template::from("{{chains.chain1}}")
                .render_chunks(&context)
                .await,
            vec![TemplateChunk::Rendered {
                value: "hello!".into(),
                sensitive: true
            }]
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
            "Error accessing environment variable `UNKNOWN`"
        );
    }

    /// Test rendering into individual chunks with complex unicode
    #[tokio::test]
    async fn test_render_chunks() {
        let profile = indexmap! { "user_id".into() => "🧡💛".into() };
        let context = create!(TemplateContext, profile: profile);

        let chunks =
            Template::from("intro {{user_id}} 💚💙💜 {{unknown}} outro")
                .render_chunks(&context)
                .await;
        assert_eq!(
            chunks,
            vec![
                TemplateChunk::Raw(Span::new(0, 6)),
                TemplateChunk::Rendered {
                    value: "🧡💛".into(),
                    sensitive: false
                },
                // Each emoji is 4 bytes
                TemplateChunk::Raw(Span::new(17, 14)),
                TemplateChunk::Error(TemplateError::FieldUnknown {
                    field: "unknown".into()
                }),
                TemplateChunk::Raw(Span::new(42, 6)),
            ]
        );
    }

    /// Helper for rendering a string
    macro_rules! render {
        ($template:expr, $context:expr) => {
            Template::from($template).render_stitched(&$context).await
        };
    }

    use render;
}
