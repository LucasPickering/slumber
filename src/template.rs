mod error;
mod parse;
mod prompt;
mod render;

pub use error::{ChainError, TemplateError, TriggeredRequestError};
pub use parse::Span;
pub use prompt::{Prompt, Prompter};

use crate::{
    collection::{Collection, ProfileId},
    db::CollectionDatabase,
    http::HttpEngine,
    template::{
        error::TemplateParseError,
        parse::{TemplateInputChunk, CHAIN_PREFIX, ENV_PREFIX},
    },
};
use derive_more::{Deref, Display};
use indexmap::IndexMap;
use serde::Serialize;
use std::{fmt::Debug, sync::atomic::AtomicU8};

/// Maximum number of layers of nested templates
const RECURSION_LIMIT: u8 = 10;

/// A little container struct for all the data that the user can access via
/// templating. Unfortunately this has to own all data so templating can be
/// deferred into a task (tokio requires `'static` for spawned tasks). If this
/// becomes a bottleneck, we can `Arc` some stuff.
#[derive(Debug)]
pub struct TemplateContext {
    /// Entire request collection
    pub collection: Collection,
    /// ID of the profile whose data should be used for rendering. Generally
    /// the caller should check the ID is valid before passing it, to
    /// provide a better error to the user if not.
    pub selected_profile: Option<ProfileId>,
    /// HTTP engine used to executed triggered sub-requests. This should only
    /// be populated if you actually want to trigger requests! In some cases
    /// you want renders to be idempotent, in which case you should pass
    /// `None`.
    pub http_engine: Option<HttpEngine>,
    /// Needed for accessing response bodies for chaining
    pub database: CollectionDatabase,
    /// Additional key=value overrides passed directly from the user
    pub overrides: IndexMap<String, String>,
    /// A conduit to ask the user questions
    pub prompter: Box<dyn Prompter>,
    /// A count of how many templates have *already* been rendered with this
    /// context. This is used to prevent infinite recursion in templates. For
    /// all external calls, you can start this at 0.
    ///
    /// This tracks the *total* number of recursive calls in a render tree, not
    /// the number of *layers*. That means one template that renders 5 child
    /// templates is the same as a template that renders a single child 5
    /// times.
    pub recursion_count: AtomicU8,
}

/// An immutable string that can contain templated content. The string is parsed
/// during creation to identify template keys, hence the immutability.
#[derive(Clone, Debug, Deref, Display, Serialize)]
#[cfg_attr(test, derive(PartialEq))]
#[display("{template}")]
#[serde(into = "String", try_from = "String")]
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
    /// If you try to render this thing, you'll always get the raw string back.
    /// The "correct" thing to do would be to add some safeguards to make that
    /// impossible (either type state or a runtime check), but it's not worth
    /// the extra code for something that is very unlikely to happen. It says
    /// "dangerous", don't be stupid.
    pub(crate) fn dangerous(template: String) -> Self {
        // Create one raw chunk for everything
        let chunk = TemplateInputChunk::Raw(Span::new(0, template.len()));
        Self {
            template,
            chunks: vec![chunk],
        }
    }
}

/// For deserialization
impl TryFrom<String> for Template {
    type Error = TemplateParseError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

/// For serialization
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
        collection::{Chain, ChainRequestTrigger, ChainSource, RecipeId},
        config::Config,
        http::{ContentType, RequestRecord},
        test_util::*,
        util::assert_err,
    };
    use chrono::Utc;
    use factori::create;
    use indexmap::indexmap;
    use rstest::rstest;
    use serde_json::json;
    use std::{env, time::Duration};
    use tokio::fs;

    /// Test overriding all key types, as well as missing keys
    #[tokio::test]
    async fn test_override() {
        let profile_data = indexmap! {"field1".into() => "field".into()};
        let source = ChainSource::Command {
            command: ["echo", "chain"]
                .iter()
                .cloned()
                .map(String::from)
                .collect(),
        };
        let overrides = indexmap! {
            "field1".into() => "override".into(),
            "chains.chain1".into() => "override".into(),
            "env.ENV1".into() => "override".into(),
            "override1".into() => "override".into(),
        };
        let profile = create!(Profile, data: profile_data);
        let profile_id = profile.id.clone();
        let chain = create!(Chain, source: source);
        let context = create!(
            TemplateContext,
            collection: create!(
                Collection,
                profiles: indexmap!{profile_id.clone() => profile},
                chains: indexmap! {chain.id.clone() => chain},
            ),
            selected_profile: Some(profile_id),
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
        let profile_data = indexmap! {
            "user_id".into() => "1".into(),
            "group_id".into() => "3".into(),
            "recursive".into() => nested_template,
        };
        let profile = create!(Profile, data: profile_data);
        let profile_id = profile.id.clone();
        let context = create!(
            TemplateContext,
            collection: create!(
                Collection,
                profiles: indexmap!{profile_id.clone() => profile},
            ),
            selected_profile: Some(profile_id),
        );

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
    #[rstest]
    #[case("{{onion_id}}", "Unknown field `onion_id`")]
    #[case(
        "{{nested}}",
        "Error in nested template `{{onion_id}}`: Unknown field `onion_id`"
    )]
    #[case("{{recursive}}", "Template recursion limit reached")]
    #[tokio::test]
    async fn test_field_error(#[case] template: &str, #[case] expected: &str) {
        let profile_data = indexmap! {
            "nested".into() => Template::parse("{{onion_id}}".into()).unwrap(),
            "recursive".into() => Template::parse("{{recursive}}".into()).unwrap(),
        };
        let profile = create!(Profile, data: profile_data);
        let profile_id = profile.id.clone();
        let context = create!(
            TemplateContext,
            collection: create!(
                Collection,
                profiles: indexmap!{profile_id.clone() => profile},
            ),
            selected_profile: Some(profile_id),
        );
        assert_err!(render!(template, context), expected);
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
        let recipe_id: RecipeId = "recipe1".into();
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
            .insert_request(&create!(
                RequestRecord,
                request: request.into(),
                response: response,
            ))
            .unwrap();
        let selector = selector.map(|s| s.parse().unwrap());
        let recipe = create!(Recipe, id: recipe_id.clone());
        let chain = create!(
            Chain,
            source: ChainSource::Request {
                recipe: recipe_id.clone(),
                trigger: Default::default(),
            },
            selector: selector,
            content_type: Some(ContentType::Json),
        );
        let context = create!(
            TemplateContext,
            collection: create!(
                Collection,
                recipes: indexmap! {recipe.id.clone() => recipe}.into(),
                chains: indexmap! {chain.id.clone() => chain},
            ),
            database: database,
        );

        assert_eq!(
            render!("{{chains.chain1}}", context).unwrap(),
            expected_value
        );
    }

    /// Test all possible error cases for chained requests. This covers all
    /// chain-specific error variants
    #[rstest]
    // Referenced a chain that doesn't exist
    #[case("unknown", create!(Chain), None, None, "Unknown chain")]
    // Chain references a recipe that's not in the collection
    #[case(
        "chain1",
        create!(
            Chain,
            source: ChainSource::Request {
                recipe: "unknown".into(),
                trigger: Default::default(),
            }
        ),
        None,
        None,
        "Unknown request recipe",
    )]
    // Recipe exists but has no history in the DB
    #[case(
        "chain1",
        create!(
            Chain,
            source: ChainSource::Request {
                recipe: "recipe1".into(),
                trigger: Default::default(),
            }
        ),
        Some("recipe1"),
        None,
        "No response available",
    )]
    // Subrequest can't be executed because triggers are disabled
    #[case(
        "chain1",
        create!(
            Chain,
            source: ChainSource::Request {
                recipe: "recipe1".into(),
                trigger: ChainRequestTrigger::Always,
            }
        ),
        Some("recipe1"),
        None,
        "Triggered request execution not allowed in this context",
    )]
    // Response doesn't include a hint to its content type
    #[case(
        "chain1",
        create!(
            Chain,
            source: ChainSource::Request {
                recipe: "recipe1".into(),
                trigger: Default::default(),
            },
            selector: Some("$.message".parse().unwrap()),
        ),
        Some("recipe1"),
        Some(create!(
            RequestRecord,
            response: create!(Response, body: "not json!".into()),
        )),
        "content type not provided",
    )]
    // Response can't be parsed according to the content type we gave
    #[case(
        "chain1",
        create!(
            Chain,
            source: ChainSource::Request {
                recipe: "recipe1".into(),
                trigger: Default::default(),
            },
            selector: Some("$.message".parse().unwrap()),
            content_type: Some(ContentType::Json),
        ),
        Some("recipe1"),
        Some(create!(
            RequestRecord,
            response: create!(Response, body: "not json!".into()),
        )),
        "Error parsing response",
    )]
    // Query returned multiple results
    #[case(
        "chain1",
        create!(
            Chain,
            source: ChainSource::Request {
                recipe: "recipe1".into(),
                trigger: Default::default(),
            },
            selector: Some("$.*".parse().unwrap()),
            content_type: Some(ContentType::Json),
        ),
        Some("recipe1"),
        Some(create!(
            RequestRecord,
            response: create!(Response, body: "[1, 2]".into()),
        )),
        "Expected exactly one result",
    )]
    #[tokio::test]
    async fn test_chain_request_error(
        #[case] chain_id: &str,
        #[case] chain: Chain,
        // ID of a recipe to add to the collection
        #[case] recipe_id: Option<&str>,
        // Optional request/response data to store in the database
        #[case] record: Option<RequestRecord>,
        #[case] expected_error: &str,
    ) {
        let database = CollectionDatabase::testing();

        let mut recipes = IndexMap::new();
        if let Some(recipe_id) = recipe_id {
            let recipe_id: RecipeId = recipe_id.into();
            recipes.insert(recipe_id.clone(), create!(Recipe, id: recipe_id));
        }

        // Insert record into DB
        if let Some(record) = record {
            database.insert_request(&record).unwrap();
        }

        let chains = indexmap! {chain_id.into() => chain};
        let context = create!(
            TemplateContext,
            collection: create!(
                Collection, recipes: recipes.into(), chains: chains
            ),
            database: database,
        );

        assert_err!(render!("{{chains.chain1}}", context), expected_error);
    }

    /// Test triggered sub-requests. We expect all of these *to trigger*
    #[rstest]
    #[case(ChainRequestTrigger::NoHistory, None)]
    #[case(ChainRequestTrigger::Expire(Duration::from_secs(0)), None)]
    #[case(
        ChainRequestTrigger::Expire(Duration::from_secs(60)),
        Some(create!(
            RequestRecord,
            end_time: Utc::now() - Duration::from_secs(100)
        ))
    )]
    #[case(ChainRequestTrigger::Always, None)]
    #[case(ChainRequestTrigger::Always, Some(create!(RequestRecord)))]
    #[tokio::test]
    async fn test_triggered_request(
        #[case] trigger: ChainRequestTrigger,
        // Optional request data to store in the database
        #[case] record: Option<RequestRecord>,
    ) {
        let database = CollectionDatabase::testing();

        // Set up DB
        if let Some(record) = record {
            database.insert_request(&record).unwrap();
        }

        // Mock HTTP response
        let mut server = mockito::Server::new_async().await;
        let url = server.url();
        let mock = server
            .mock("GET", "/get")
            .with_status(201)
            .with_body("hello!")
            .create_async()
            .await;

        let recipe = create!(Recipe, url: format!("{url}/get").as_str().into());
        let chain = create!(
            Chain,
            source: ChainSource::Request {
                recipe: recipe.id.clone(),
                trigger,
            },
        );
        let http_engine = HttpEngine::new(&Config::default(), database.clone());
        let context = create!(
            TemplateContext,
            collection: create!(
                Collection,
                recipes: indexmap! {recipe.id.clone() => recipe}.into(),
                chains: indexmap! {chain.id.clone() => chain},
            ),
            http_engine: Some(http_engine),
            database: database,
        );

        assert_eq!(render!("{{chains.chain1}}", context).unwrap(), "hello!");

        mock.assert();
    }

    /// Test success with chained command
    #[tokio::test]
    async fn test_chain_command() {
        let command = vec!["echo".into(), "-n".into(), "hello!".into()];
        let chain = create!(Chain, source: ChainSource::Command{command});
        let context = create!(
            TemplateContext,
            collection: create!(
                Collection,
                chains: indexmap! {chain.id.clone() => chain},
            ),
        );

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
        let source = ChainSource::Command {
            command: command.iter().cloned().map(String::from).collect(),
        };
        let chain = create!(Chain, source: source);
        let context = create!(
            TemplateContext,
            collection: create!(
                Collection,
                chains: indexmap! {chain.id.clone() => chain},
            ),
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

        let chain =
            create!(Chain, source: ChainSource::File { path: file_path });
        let context = create!(
            TemplateContext,
            collection: create!(
                Collection,
                chains: indexmap! {chain.id.clone() => chain},
            ),
        );

        assert_eq!(render!("{{chains.chain1}}", context).unwrap(), "hello!");
    }

    /// Test failure with chained file
    #[tokio::test]
    async fn test_chain_file_error() {
        let chain = create!(Chain, source: ChainSource::File { path: "not-a-real-file".into() });
        let context = create!(
            TemplateContext,
            collection: create!(
                Collection,
                chains: indexmap! {chain.id.clone() => chain},
            ),
        );

        assert_err!(
            render!("{{chains.chain1}}", context),
            "Error reading from file"
        );
    }

    #[tokio::test]
    async fn test_chain_prompt() {
        let chain = create!(
            Chain,
            source: ChainSource::Prompt { message: Some("password".into()) },
        );
        let context = create!(
            TemplateContext,
            collection: create!(
                Collection,
                chains: indexmap! {chain.id.clone() => chain},
            )
            prompter: Box::new(TestPrompter::new(Some("hello!"))),
        );

        assert_eq!(render!("{{chains.chain1}}", context).unwrap(), "hello!");
    }

    /// Prompting gone wrong
    #[tokio::test]
    async fn test_chain_prompt_error() {
        let chain = create!(
            Chain,
            source: ChainSource::Prompt { message: Some("password".into()) },
        );
        let context = create!(
            TemplateContext,
            collection: create!(
                Collection,
                chains: indexmap! {chain.id.clone() => chain},
            ),
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
        let chain = create!(
            Chain,
            source: ChainSource::Prompt { message: Some("password".into()) },
            sensitive: true,
        );
        let context = create!(
            TemplateContext,
            collection: create!(
                Collection,
                chains: indexmap! {chain.id.clone() => chain},
            ),
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
        let profile_data = indexmap! { "user_id".into() => "🧡💛".into() };
        let profile = create!(Profile, data: profile_data);
        let profile_id = profile.id.clone();
        let context = create!(
            TemplateContext,
            collection: create!(
                Collection,
                profiles: indexmap!{profile_id.clone() => profile},
            ),
            selected_profile: Some(profile_id),
        );

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
