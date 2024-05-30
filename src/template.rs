mod error;
mod parse;
mod prompt;
mod render;

pub use error::{ChainError, TemplateError};
pub use parse::Span;
pub use prompt::{Prompt, PromptChannel, Prompter};

use crate::{
    collection::{Collection, ProfileId},
    db::CollectionDatabase,
    http::HttpEngine,
    template::{
        error::TemplateParseError,
        parse::{TemplateInputChunk, CHAIN_PREFIX, ENV_PREFIX},
    },
};
use derive_more::Display;
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
#[derive(Clone, Debug, Display, Serialize)]
#[cfg_attr(test, derive(PartialEq))]
#[display("{template}")]
#[serde(into = "String", try_from = "String")]
pub struct Template {
    template: String,
    /// Pre-parsed chunks of the template. We can't store slices here because
    /// that would be self-referential, so just store locations. These chunks
    /// are contiguous and span the whole template.
    chunks: Vec<TemplateInputChunk<Span>>,
}

impl Template {
    /// Get the raw template text
    pub fn as_str(&self) -> &str {
        &self.template
    }

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

/// For rstest magic conversion
#[cfg(test)]
impl std::str::FromStr for Template {
    type Err = TemplateParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s.to_owned())
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
    Rendered { value: Vec<u8>, sensitive: bool },
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
impl crate::test_util::Factory for TemplateContext {
    fn factory(_: ()) -> Self {
        use crate::test_util::TestPrompter;
        Self {
            collection: Collection::default(),
            selected_profile: None,
            http_engine: None,
            database: CollectionDatabase::factory(()),
            overrides: IndexMap::new(),
            prompter: Box::<TestPrompter>::default(),
            recursion_count: 0.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        collection::{
            Chain, ChainOutputTrim, ChainRequestSection, ChainRequestTrigger,
            ChainSource, Profile, Recipe, RecipeId,
        },
        config::Config,
        http::{ContentType, Exchange, RequestRecord, ResponseRecord},
        test_util::{
            assert_err, header_map, temp_dir, Factory, TempDir, TestPrompter,
        },
    };
    use chrono::Utc;
    use indexmap::indexmap;
    use rstest::rstest;
    use serde_json::json;
    use std::{env, time::Duration};
    use tokio::fs;

    /// Test overriding all key types, as well as missing keys
    #[tokio::test]
    async fn test_override() {
        let profile_data = indexmap! {"field1".into() => "field".into()};
        let overrides = indexmap! {
            "field1".into() => "override".into(),
            "chains.chain1".into() => "override".into(),
            "env.ENV1".into() => "override".into(),
            "override1".into() => "override".into(),
        };
        let profile = Profile {
            data: profile_data,
            ..Profile::factory(())
        };
        let profile_id = profile.id.clone();
        let chain = Chain {
            source: ChainSource::command(["echo", "chain"]),
            ..Chain::factory(())
        };
        let context = TemplateContext {
            collection: Collection {
                profiles: indexmap! {profile_id.clone() => profile},
                chains: indexmap! {chain.id.clone() => chain},
                ..Collection::factory(())
            },
            selected_profile: Some(profile_id),
            overrides,
            ..TemplateContext::factory(())
        };

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
        let profile = Profile {
            data: profile_data,
            ..Profile::factory(())
        };
        let profile_id = profile.id.clone();
        let context = TemplateContext {
            collection: Collection {
                profiles: indexmap! {profile_id.clone() => profile},
                ..Collection::factory(())
            },
            selected_profile: Some(profile_id),
            ..TemplateContext::factory(())
        };

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
    #[case::unknown_field("{{onion_id}}", "Unknown field `onion_id`")]
    #[case::nested(
        "{{nested}}",
        "Rendering nested template for field `nested`: \
        Unknown field `onion_id`"
    )]
    #[case::recursion_limit(
        "{{recursive}}",
        "Template recursion limit reached"
    )]
    #[tokio::test]
    async fn test_field_error(#[case] template: &str, #[case] expected: &str) {
        let profile_data = indexmap! {
            "nested".into() => Template::parse("{{onion_id}}".into()).unwrap(),
            "recursive".into() => Template::parse("{{recursive}}".into()).unwrap(),
        };
        let profile = Profile {
            data: profile_data,
            ..Profile::factory(())
        };
        let profile_id = profile.id.clone();
        let context = TemplateContext {
            collection: Collection {
                profiles: indexmap! {profile_id.clone() => profile},
                ..Collection::factory(())
            },
            selected_profile: Some(profile_id),
            ..TemplateContext::factory(())
        };
        assert_err!(render!(template, context), expected);
    }

    /// Test success cases with chained responses
    #[rstest]
    #[case::no_selector(
        None,
        ChainRequestSection::Body,
        r#"{"array":[1,2],"bool":false,"number":6,"object":{"a":1},"string":"Hello World!"}"#
    )]
    #[case::string(Some("$.string"), ChainRequestSection::Body, "Hello World!")]
    #[case::number(Some("$.number"), ChainRequestSection::Body, "6")]
    #[case::bool(Some("$.bool"), ChainRequestSection::Body, "false")]
    #[case::array(Some("$.array"), ChainRequestSection::Body, "[1,2]")]
    #[case::object(Some("$.object"), ChainRequestSection::Body, "{\"a\":1}")]
    #[case::header(None, ChainRequestSection::Header("Token".into()), "Secret Value")]
    #[tokio::test]
    async fn test_chain_request(
        #[case] selector: Option<&str>,
        #[case] section: ChainRequestSection,
        #[case] expected_value: &str,
    ) {
        let recipe_id: RecipeId = "recipe1".into();
        let database = CollectionDatabase::factory(());
        let response_body = json!({
            "string": "Hello World!",
            "number": 6,
            "bool": false,
            "array": [1,2],
            "object": {"a": 1},
        });
        let response_headers =
            header_map(indexmap! {"Token" => "Secret Value"});
        let request = RequestRecord {
            recipe_id: recipe_id.clone(),
            ..RequestRecord::factory(())
        };
        let response = ResponseRecord {
            body: response_body.to_string().into_bytes().into(),
            headers: response_headers,
            ..ResponseRecord::factory(())
        };
        database
            .insert_exchange(&Exchange {
                request: request.into(),
                response: response.into(),
                ..Exchange::factory(())
            })
            .unwrap();
        let selector = selector.map(|s| s.parse().unwrap());
        let recipe = Recipe {
            id: recipe_id.clone(),
            ..Recipe::factory(())
        };
        let chain = Chain {
            source: ChainSource::Request {
                recipe: recipe_id.clone(),
                trigger: Default::default(),
                section,
            },
            selector,
            content_type: Some(ContentType::Json),
            ..Chain::factory(())
        };
        let context = TemplateContext {
            collection: Collection {
                recipes: indexmap! {recipe.id.clone() => recipe}.into(),
                chains: indexmap! {chain.id.clone() => chain},
                ..Collection::factory(())
            },
            database,
            ..TemplateContext::factory(())
        };

        assert_eq!(
            render!("{{chains.chain1}}", context).unwrap(),
            expected_value
        );
    }

    /// Test all possible error cases for chained requests. This covers all
    /// chain-specific error variants
    #[rstest]
    // Referenced a chain that doesn't exist
    #[case::unknown_chain(
        "unknown",
        Chain::factory(()),
        None,
        None,
        "Unknown chain"
    )]
    // Chain references a recipe that's not in the collection
    #[case::unknown_recipe(
        "chain1",
        Chain {
            source: ChainSource::Request {
                recipe: "unknown".into(),
                trigger: Default::default(),
                section: Default::default(),
            },
            ..Chain::factory(())
        },
        None,
        None,
        "Unknown request recipe",
    )]
    // Recipe exists but has no history in the DB
    #[case::no_response(
        "chain1",
        Chain {
            source: ChainSource::Request {
                recipe: "recipe1".into(),
                trigger: Default::default(),
                section: Default::default(),
            },
            ..Chain::factory(())
        },
        Some("recipe1"),
        None,
        "No response available",
    )]
    // Subrequest can't be executed because triggers are disabled
    #[case::trigger_disabled(
        "chain1",
        Chain {
            source: ChainSource::Request {
                recipe: "recipe1".into(),
                trigger: ChainRequestTrigger::Always,
                section: Default::default(),
            },
            ..Chain::factory(())
        },
        Some("recipe1"),
        None,
        "Triggered request execution not allowed in this context",
    )]
    // Response doesn't include a hint to its content type
    #[case::no_content_type(
        "chain1",
        Chain {
            source: ChainSource::Request {
                recipe: "recipe1".into(),
                trigger: Default::default(),
                section: Default::default(),
            },
            selector: Some("$.message".parse().unwrap()),
            ..Chain::factory(())
        },
        Some("recipe1"),
        Some(Exchange {
            response: ResponseRecord {
                body: "not json!".into(),
                ..ResponseRecord::factory(())
            }.into(),
            ..Exchange::factory(())
        }),
        "content type not provided",
    )]
    // Response can't be parsed according to the content type we gave
    #[case::parse_response(
        "chain1",
        Chain {
            source: ChainSource::Request {
                recipe: "recipe1".into(),
                trigger: Default::default(),
                section: Default::default(),
            },
            selector: Some("$.message".parse().unwrap()),
            content_type: Some(ContentType::Json),
            ..Chain::factory(())
        },
        Some("recipe1"),
        Some(Exchange {
            response: ResponseRecord {
                body: "not json!".into(),
                ..ResponseRecord::factory(())
            }.into(),
            ..Exchange::factory(())
        }),
        "Parsing response: expected ident at line 1 column 2",
    )]
    // Query returned multiple results
    #[case::query_multiple_results(
        "chain1",
        Chain {
            source: ChainSource::Request {
                recipe: "recipe1".into(),
                trigger: Default::default(),
                section:Default::default()
            },
            selector: Some("$.*".parse().unwrap()),
            content_type: Some(ContentType::Json),
            ..Chain::factory(())
        },
        Some("recipe1"),
        Some(Exchange {
            response: ResponseRecord {
                body: "[1, 2]".into(),
                ..ResponseRecord::factory(())
            }.into(),
            ..Exchange::factory(())
        }),
        "Expected exactly one result",
    )]
    #[tokio::test]
    async fn test_chain_request_error(
        #[case] chain_id: &str,
        #[case] chain: Chain,
        // ID of a recipe to add to the collection
        #[case] recipe_id: Option<&str>,
        // Optional request/response data to store in the database
        #[case] exchange: Option<Exchange>,
        #[case] expected_error: &str,
    ) {
        let database = CollectionDatabase::factory(());

        let mut recipes = IndexMap::new();
        if let Some(recipe_id) = recipe_id {
            let recipe_id: RecipeId = recipe_id.into();
            recipes.insert(
                recipe_id.clone(),
                Recipe {
                    id: recipe_id,
                    ..Recipe::factory(())
                },
            );
        }

        // Insert exchange into DB
        if let Some(exchange) = exchange {
            database.insert_exchange(&exchange).unwrap();
        }

        let chains = indexmap! {chain_id.into() => chain};
        let context = TemplateContext {
            collection: Collection {
                recipes: recipes.into(),
                chains,
                ..Collection::factory(())
            },
            database,
            ..TemplateContext::factory(())
        };

        assert_err!(render!("{{chains.chain1}}", context), expected_error);
    }

    /// Test triggered sub-requests. We expect all of these *to trigger*
    #[rstest]
    #[case::no_history(ChainRequestTrigger::NoHistory, None)]
    #[case::expire_empty(
        ChainRequestTrigger::Expire(Duration::from_secs(0)),
        None
    )]
    #[case::expire_with_duration(
        ChainRequestTrigger::Expire(Duration::from_secs(60)),
        Some(Exchange {
            end_time: Utc::now() - Duration::from_secs(100),
            ..Exchange::factory(())})
    )]
    #[case::always_no_history(ChainRequestTrigger::Always, None)]
    #[case::always_with_history(
        ChainRequestTrigger::Always,
        Some(Exchange::factory(()))
    )]
    #[tokio::test]
    async fn test_triggered_request(
        #[case] trigger: ChainRequestTrigger,
        // Optional request data to store in the database
        #[case] exchange: Option<Exchange>,
    ) {
        let database = CollectionDatabase::factory(());

        // Set up DB
        if let Some(exchange) = exchange {
            database.insert_exchange(&exchange).unwrap();
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

        let recipe = Recipe {
            url: format!("{url}/get").as_str().into(),
            ..Recipe::factory(())
        };
        let chain = Chain {
            source: ChainSource::Request {
                recipe: recipe.id.clone(),
                trigger,
                section: Default::default(),
            },
            ..Chain::factory(())
        };
        let http_engine = HttpEngine::new(&Config::default());
        let context = TemplateContext {
            collection: Collection {
                recipes: indexmap! {recipe.id.clone() => recipe}.into(),
                chains: indexmap! {chain.id.clone() => chain},
                ..Collection::factory(())
            },
            http_engine: Some(http_engine),
            database,
            ..TemplateContext::factory(())
        };

        assert_eq!(render!("{{chains.chain1}}", context).unwrap(), "hello!");

        mock.assert();
    }

    /// Test success with chained command
    #[rstest]
    #[case::with_stdin(&["tail"], Some("hello!"), "hello!")]
    #[case::raw_command(&["echo", "-n", "hello!"], None, "hello!")]
    #[tokio::test]
    async fn test_chain_command(
        #[case] command: &[&str],
        #[case] stdin: Option<&str>,
        #[case] expected: &str,
    ) {
        let source = ChainSource::Command {
            command: command.iter().copied().map(Template::from).collect(),
            stdin: stdin.map(Template::from),
        };
        let chain = Chain {
            source,
            ..Chain::factory(())
        };
        let context = TemplateContext {
            collection: Collection {
                chains: indexmap! {chain.id.clone() => chain},
                ..Collection::factory(())
            },
            ..TemplateContext::factory(())
        };

        assert_eq!(render!("{{chains.chain1}}", context).unwrap(), expected);
    }

    /// Test trimmed chained command
    #[rstest]
    #[case::no_trim(ChainOutputTrim::None, "   hello!   ")]
    #[case::trim_start(ChainOutputTrim::Start, "hello!   ")]
    #[case::trim_end(ChainOutputTrim::End, "   hello!")]
    #[case::trim_both(ChainOutputTrim::Both, "hello!")]
    #[tokio::test]
    async fn test_chain_output_trim(
        #[case] trim: ChainOutputTrim,
        #[case] expected: &str,
    ) {
        let chain = Chain {
            source: ChainSource::command(["echo", "-n", "   hello!   "]),
            trim,
            ..Chain::factory(())
        };
        let context = TemplateContext {
            collection: Collection {
                chains: indexmap! {chain.id.clone() => chain},
                ..Collection::factory(())
            },
            ..TemplateContext::factory(())
        };

        assert_eq!(render!("{{chains.chain1}}", context).unwrap(), expected);
    }

    /// Test failure with chained command
    #[rstest]
    #[case::no_command(&[], None, "No command given")]
    #[case::unknown_command(
        &["totally not a program"], None, "No such file or directory"
    )]
    #[case::command_error(
        &["head", "/dev/random"], None, "invalid utf-8 sequence"
    )]
    #[case::stdin_error(
        &["tail"],
        Some("{{chains.stdin}}"),
        "Resolving chain `chain1`: Rendering nested template for field `stdin`: \
         Resolving chain `stdin`: Unknown chain: stdin"
    )]
    #[tokio::test]
    async fn test_chain_command_error(
        #[case] command: &[&str],
        #[case] stdin: Option<&str>,
        #[case] expected_error: &str,
    ) {
        let source = ChainSource::Command {
            command: command.iter().copied().map(Template::from).collect(),
            stdin: stdin.map(Template::from),
        };
        let chain = Chain {
            source,
            ..Chain::factory(())
        };
        let context = TemplateContext {
            collection: Collection {
                chains: indexmap! {chain.id.clone() => chain},
                ..Collection::factory(())
            },
            ..TemplateContext::factory(())
        };

        assert_err!(render!("{{chains.chain1}}", context), expected_error);
    }

    /// Test success with chained file
    #[rstest]
    #[tokio::test]
    async fn test_chain_file(temp_dir: TempDir) {
        // Create a temp file that we'll read from
        let path = temp_dir.join("stuff.txt");
        fs::write(&path, "hello!").await.unwrap();
        // Sanity check to debug race condition
        assert_eq!(fs::read_to_string(&path).await.unwrap(), "hello!");
        let path: Template = path.to_str().unwrap().into();

        let chain = Chain {
            source: ChainSource::File { path: path.clone() },
            ..Chain::factory(())
        };
        let context = TemplateContext {
            collection: Collection {
                chains: indexmap! {chain.id.clone() => chain},
                ..Collection::factory(())
            },
            ..TemplateContext::factory(())
        };

        assert_eq!(
            render!("{{chains.chain1}}", context).unwrap(),
            "hello!",
            "{path:?}"
        );
    }

    /// Test failure with chained file
    #[tokio::test]
    async fn test_chain_file_error() {
        let chain = Chain {
            source: ChainSource::File {
                path: "not-real".into(),
            },
            ..Chain::factory(())
        };
        let context = TemplateContext {
            collection: Collection {
                chains: indexmap! {chain.id.clone() => chain},
                ..Collection::factory(())
            },
            ..TemplateContext::factory(())
        };

        assert_err!(
            render!("{{chains.chain1}}", context),
            "Reading file `not-real`"
        );
    }

    #[tokio::test]
    async fn test_chain_prompt() {
        let chain = Chain {
            source: ChainSource::Prompt {
                message: Some("password".into()),
                default: Some("default".into()),
            },
            ..Chain::factory(())
        };

        // Test value from prompter
        let mut context = TemplateContext {
            collection: Collection {
                chains: indexmap! {chain.id.clone() => chain},
                ..Collection::factory(())
            },

            prompter: Box::new(TestPrompter::new(Some("hello!"))),
            ..TemplateContext::factory(())
        };
        assert_eq!(render!("{{chains.chain1}}", context).unwrap(), "hello!");

        // Test default value
        context.prompter = Box::new(TestPrompter::new::<String>(None));
        assert_eq!(render!("{{chains.chain1}}", context).unwrap(), "default");
    }

    /// Prompting gone wrong
    #[tokio::test]
    async fn test_chain_prompt_error() {
        let chain = Chain {
            source: ChainSource::Prompt {
                message: Some("password".into()),
                default: None,
            },
            ..Chain::factory(())
        };
        let context = TemplateContext {
            collection: Collection {
                chains: indexmap! {chain.id.clone() => chain},
                ..Collection::factory(())
            },
            // Prompter gives no response
            prompter: Box::new(TestPrompter::new::<String>(None)),
            ..TemplateContext::factory(())
        };

        assert_err!(
            render!("{{chains.chain1}}", context),
            "No response from prompt"
        );
    }

    /// Values marked sensitive should have that flag set in the rendered output
    #[tokio::test]
    async fn test_chain_sensitive() {
        let chain = Chain {
            source: ChainSource::Prompt {
                message: Some("password".into()),
                default: None,
            },
            sensitive: true,
            ..Chain::factory(())
        };
        let context = TemplateContext {
            collection: Collection {
                chains: indexmap! {chain.id.clone() => chain},
                ..Collection::factory(())
            },
            // Prompter gives no response
            prompter: Box::new(TestPrompter::new(Some("hello!"))),
            ..TemplateContext::factory(())
        };
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

    /// Test linking two chains together. This example is contribed because the
    /// command could just read the file itself, but don't worry about it it's
    /// just a test.
    #[rstest]
    #[tokio::test]
    async fn test_chain_nested(temp_dir: TempDir) {
        // Chain 1 - file
        let path = temp_dir.join("stuff.txt");
        fs::write(&path, "hello!").await.unwrap();
        let path: Template = path.to_str().unwrap().into();
        let file_chain = Chain {
            id: "file".into(),
            source: ChainSource::File { path },
            ..Chain::factory(())
        };

        // Chain 2 - command
        let command_chain = Chain {
            id: "command".into(),
            source: ChainSource::command([
                "echo",
                "-n",
                "answer: {{chains.file}}",
            ]),
            ..Chain::factory(())
        };

        let context = TemplateContext {
            collection: Collection {
                chains: indexmap! {
                    file_chain.id.clone() => file_chain,
                    command_chain.id.clone() => command_chain,
                },
                ..Collection::factory(())
            },
            ..TemplateContext::factory(())
        };
        assert_eq!(
            render!("{{chains.command}}", context).unwrap(),
            "answer: hello!"
        );
    }

    /// Test when an error occurs in a nested chain
    #[tokio::test]
    async fn test_chain_nested_error() {
        // Chain 1 - file
        let file_chain = Chain {
            id: "file".into(),
            source: ChainSource::File {
                path: "bogus.txt".into(),
            },

            ..Chain::factory(())
        };

        // Chain 2 - command
        let command_chain = Chain {
            id: "command".into(),
            source: ChainSource::command([
                "echo",
                "-n",
                "answer: {{chains.file}}",
            ]),
            ..Chain::factory(())
        };

        let context = TemplateContext {
            collection: Collection {
                chains: indexmap! {
                    file_chain.id.clone() => file_chain,
                    command_chain.id.clone() => command_chain,
                },
                ..Collection::factory(())
            },
            ..TemplateContext::factory(())
        };
        assert_err!(
            render!("{{chains.command}}", context),
            "Rendering nested template for field `command[2]`: \
            Resolving chain `file`: Reading file `bogus.txt`: \
            No such file or directory"
        );
    }

    #[tokio::test]
    async fn test_environment_success() {
        let context = TemplateContext::factory(());
        env::set_var("TEST", "test!");
        assert_eq!(render!("{{env.TEST}}", context).unwrap(), "test!");
        // Unknown gets replaced with empty string
        assert_eq!(render!("{{env.UNKNOWN}}", context).unwrap(), "");
    }

    /// Test rendering non-UTF-8 data
    #[tokio::test]
    async fn test_render_binary() {
        let chain = Chain {
            source: ChainSource::command(["echo", "-n", "-e", r#"\xc3\x28"#]),
            ..Chain::factory(())
        };
        let context = TemplateContext {
            collection: Collection {
                chains: indexmap! {chain.id.clone() => chain},
                ..Collection::factory(())
            },
            ..TemplateContext::factory(())
        };

        assert_eq!(
            Template::from("{{chains.chain1}}")
                .render(&context)
                .await
                .unwrap(),
            b"\xc3\x28"
        );
    }

    /// Test rendering non-UTF-8 data to string returns an error
    #[tokio::test]
    async fn test_render_invalid_utf8() {
        let chain = Chain {
            source: ChainSource::command(["echo", "-n", "-e", r#"\xc3\x28"#]),
            ..Chain::factory(())
        };
        let context = TemplateContext {
            collection: Collection {
                chains: indexmap! {chain.id.clone() => chain},
                ..Collection::factory(())
            },
            ..TemplateContext::factory(())
        };

        assert_err!(render!("{{chains.chain1}}", context), "invalid utf-8");
    }

    /// Test rendering into individual chunks with complex unicode
    #[tokio::test]
    async fn test_render_chunks() {
        let profile_data = indexmap! { "user_id".into() => "🧡💛".into() };
        let profile = Profile {
            data: profile_data,
            ..Profile::factory(())
        };
        let profile_id = profile.id.clone();
        let context = TemplateContext {
            collection: Collection {
                profiles: indexmap! {profile_id.clone() => profile},
                ..Collection::factory(())
            },
            selected_profile: Some(profile_id),
            ..TemplateContext::factory(())
        };

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

    /// Helper for rendering a template to a string
    macro_rules! render {
        ($template:expr, $context:expr) => {
            Template::from($template).render_string(&$context).await
        };
    }
    use render;
}
