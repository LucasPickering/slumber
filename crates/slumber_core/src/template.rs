//! Generate strings (and bytes) from user-written templates with dynamic data

mod cereal;
mod error;
mod parse;
mod prompt;
mod render;

pub use error::{ChainError, TemplateError, TriggeredRequestError};
pub use prompt::{Prompt, PromptChannel, Prompter, Select};

use crate::{
    collection::{ChainId, Collection, ProfileId},
    db::CollectionDatabase,
    http::HttpEngine,
    template::{
        parse::{TemplateInputChunk, CHAIN_PREFIX, ENV_PREFIX},
        render::RenderGroupState,
    },
};
use derive_more::{Deref, Display};
use indexmap::IndexMap;
#[cfg(test)]
use proptest::{arbitrary::any, strategy::Strategy};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// A parsed template, which can contain raw and/or templated content. The
/// string is parsed during creation to identify template keys, hence the
/// immutability.
///
/// The original string is *not* stored. To recover the source string, use the
/// [Display] implementation.
///
/// Invariants:
/// - Two templates with the same source string will have the same set of
///   chunks, and vice versa
/// - No two raw segments will ever be consecutive
#[derive(Clone, Debug, Default, PartialEq)]
#[cfg_attr(test, derive(proptest_derive::Arbitrary))]
pub struct Template {
    /// Pre-parsed chunks of the template. For raw chunks we store the
    /// presentation text (which is not necessarily the source text, as escape
    /// sequences will be eliminated). For keys, just store the needed
    /// metadata.
    #[cfg_attr(
        test,
        proptest(
            strategy = "any::<Vec<TemplateInputChunk>>().prop_map(join_raw)"
        )
    )]
    chunks: Vec<TemplateInputChunk>,
}

/// A little container struct for all the data that the user can access via
/// templating. Unfortunately this has to own all data so templating can be
/// deferred into a task (tokio requires `'static` for spawned tasks). If this
/// becomes a bottleneck, we can `Arc` some stuff.
#[derive(Debug)]
pub struct TemplateContext {
    /// Entire request collection
    pub collection: Arc<Collection>,
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
    /// State that should be shared across al renders that use this context.
    /// This is meant to be opaque; just use [Default::default] to initialize.
    pub state: RenderGroupState,
}

impl Template {
    /// Create a new template from a raw string, without parsing it at all.
    /// Useful when importing from external formats where the string isn't
    /// expected to be a valid Slumber template
    pub fn raw(template: String) -> Template {
        let chunks = if template.is_empty() {
            vec![]
        } else {
            // This may seem too easy, but the hard part comes during
            // stringification, when we need to add backslashes to get the
            // string to parse correctly later
            vec![TemplateInputChunk::Raw(template.into())]
        };
        Self { chunks }
    }

    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }
}

#[cfg(any(test, feature = "test"))]
impl From<&str> for Template {
    fn from(value: &str) -> Self {
        value.parse().unwrap()
    }
}

#[cfg(any(test, feature = "test"))]
impl From<String> for Template {
    fn from(value: String) -> Self {
        value.as_str().into()
    }
}

#[cfg(any(test, feature = "test"))]
impl From<serde_json::Value> for Template {
    fn from(value: serde_json::Value) -> Self {
        format!("{value:#}").into()
    }
}

/// An identifier that can be used in a template key. A valid identifier is
/// any non-empty string that contains only alphanumeric characters, `-`, or
/// `_`.
///
/// Construct via [FromStr](std::str::FromStr)
#[derive(
    Clone,
    Debug,
    Deref,
    Default,
    Display,
    Eq,
    Hash,
    PartialEq,
    Serialize,
    Deserialize,
)]
#[cfg_attr(test, derive(proptest_derive::Arbitrary))]
pub struct Identifier(
    #[cfg_attr(test, proptest(regex = "[a-zA-Z0-9-_]+"))] String,
);

/// A shortcut for creating identifiers from static strings. Since the string
/// is defined in code we're assuming it's valid.
impl From<&'static str> for Identifier {
    fn from(value: &'static str) -> Self {
        Self(value.parse().unwrap())
    }
}

/// A piece of a rendered template string. A collection of chunks collectively
/// constitutes a rendered string, and those chunks should be contiguous.
#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq))]
pub enum TemplateChunk {
    /// Raw unprocessed text, i.e. something **outside** the `{{ }}`. This is
    /// stored in an `Arc` so we can reference the text in the parsed input
    /// without having to clone it.
    Raw(Arc<String>),
    /// Outcome of rendering a template key
    Rendered {
        /// This is wrapped in `Arc` to de-duplicate large values derived from
        /// chains. When the same chain is used multiple times in a render
        /// group it gets deduplicated, meaning multiple render results would
        /// refer to the same data. In the vast majority of cases though we
        /// only ever have one pointer to this data. This is arguably a
        /// premature optimization, but it's very possible for a large chained
        /// body to be used twice, and we wouldn't want to duplicate that.
        value: Arc<Vec<u8>>,
        sensitive: bool,
    },
    /// An error occurred while rendering a template key
    Error(TemplateError),
}

#[cfg(test)]
impl TemplateChunk {
    /// Shorthand for creating a new raw chunk
    fn raw(value: &str) -> Self {
        Self::Raw(value.to_owned().into())
    }
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
#[derive(Clone, Debug, Display, PartialEq)]
#[cfg_attr(test, derive(proptest_derive::Arbitrary))]
pub enum TemplateKey {
    /// A plain field, which can come from the profile or an override
    Field(Identifier),
    /// A value from a predefined chain of another recipe
    #[display("{CHAIN_PREFIX}{_0}")]
    Chain(ChainId),
    /// A value pulled from the process environment
    /// DEPRECATED: To be removed in 2.0, replaced by !env chain source
    #[display("{ENV_PREFIX}{_0}")]
    Environment(Identifier),
}

#[cfg(any(test, feature = "test"))]
impl crate::test_util::Factory for TemplateContext {
    fn factory(_: ()) -> Self {
        use crate::test_util::TestPrompter;
        Self {
            collection: Default::default(),
            selected_profile: None,
            http_engine: None,
            database: CollectionDatabase::factory(()),
            overrides: IndexMap::new(),
            prompter: Box::<TestPrompter>::default(),
            state: RenderGroupState::default(),
        }
    }
}

/// Join consecutive raw chunks in a generated template, to make it valid
#[cfg(test)]
fn join_raw(chunks: Vec<TemplateInputChunk>) -> Vec<TemplateInputChunk> {
    let len = chunks.len();
    chunks
        .into_iter()
        .fold(Vec::with_capacity(len), |mut chunks, chunk| {
            match (chunks.last_mut(), chunk) {
                // If previous and current are both raw, join them together
                (
                    Some(TemplateInputChunk::Raw(previous)),
                    TemplateInputChunk::Raw(current),
                ) => {
                    // The current string is inside an Arc so we can't push
                    // into it, we have to clone it out :(
                    let mut concat =
                        String::with_capacity(previous.len() + current.len());
                    concat.push_str(previous);
                    concat.push_str(&current);
                    *previous = Arc::new(concat)
                }
                (_, chunk) => chunks.push(chunk),
            }
            chunks
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        assert_err,
        collection::{
            Chain, ChainOutputTrim, ChainRequestSection, ChainRequestTrigger,
            ChainSource, Profile, Recipe, RecipeId,
        },
        http::{
            content_type::ContentType, Exchange, RequestRecord, ResponseRecord,
        },
        test_util::{
            by_id, header_map, http_engine, invalid_utf8_chain, temp_dir,
            Factory, TempDir, TestPrompter,
        },
    };
    use chrono::Utc;
    use indexmap::indexmap;
    use rstest::rstest;
    use serde_json::json;
    use std::time::Duration;
    use tokio::fs;
    use wiremock::{matchers, Mock, MockServer, ResponseTemplate};

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
                profiles: by_id([profile]),
                chains: by_id([chain]),
                ..Collection::factory(())
            }
            .into(),
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
    #[rstest]
    #[case::empty("", "")]
    #[case::raw("plain", "plain")]
    #[case::nested("{{nested}}", "user id: 1")]
    // Using the same nested field twice should *not* trigger cycle detection
    #[case::nested_twice("{{nested}} {{nested}}", "user id: 1 user id: 1")]
    #[case::complex(
        // Test complex stitching. Emoji is important to test because the
        // stitching uses character indexes
        "start {{user_id}} 🧡💛 {{group_id}} end",
        "start 1 🧡💛 3 end"
    )]
    #[tokio::test]
    async fn test_field(#[case] template: &str, #[case] expected: &str) {
        let context = profile_context(indexmap! {
            "user_id".into() => "1".into(),
            "group_id".into() => "3".into(),
            "nested".into() => "user id: {{user_id}}".into(),
        });

        assert_eq!(&render!(template, context).unwrap(), expected);
    }

    /// Potential error cases for a profile field
    #[rstest]
    #[case::unknown_field("{{onion_id}}", "Unknown field `onion_id`")]
    #[case::nested(
        "{{nested}}",
        "Rendering nested template for field `nested`: \
        Unknown field `onion_id`"
    )]
    #[tokio::test]
    async fn test_field_error(#[case] template: &str, #[case] expected: &str) {
        let context = profile_context(indexmap! {
            "nested".into() => "{{onion_id}}".into(),
            "recursive".into() => "{{recursive}}".into(),
        });
        assert_err!(render!(template, context), expected);
    }

    /// Test success cases with chained responses
    #[rstest]
    #[case::no_selector(
        None,
        ChainRequestSection::Body,
        &json!({
            "array": [1, 2],
            "bool": false,
            "number": 6,
            "object": {"a": 1},
            "string": "Hello World!"
        }).to_string()
    )]
    #[case::string(Some("$.string"), ChainRequestSection::Body, "Hello World!")]
    #[case::number(Some("$.number"), ChainRequestSection::Body, "6")]
    #[case::bool(Some("$.bool"), ChainRequestSection::Body, "false")]
    #[case::array(Some("$.array"), ChainRequestSection::Body, "[1,2]")]
    #[case::object(Some("$.object"), ChainRequestSection::Body, "{\"a\":1}")]
    #[case::header(
        None,
        ChainRequestSection::Header("Token".into()),
        "Secret Value",
    )]
    #[case::header(
        None,
        ChainRequestSection::Header("{{header}}".into()),
        "Secret Value",
    )]
    #[tokio::test]
    async fn test_chain_request(
        #[case] selector: Option<&str>,
        #[case] section: ChainRequestSection,
        #[case] expected_value: &str,
    ) {
        let profile = Profile {
            data: indexmap! {"header".into() => "Token".into()},
            ..Profile::factory(())
        };
        let recipe = Recipe {
            ..Recipe::factory(())
        };
        let selector = selector.map(|s| s.parse().unwrap());
        let chain = Chain {
            source: ChainSource::Request {
                recipe: recipe.id.clone(),
                trigger: Default::default(),
                section,
            },
            selector,
            content_type: Some(ContentType::Json),
            ..Chain::factory(())
        };

        let database = CollectionDatabase::factory(());
        let response_body = json!({
            "string": "Hello World!",
            "number": 6,
            "bool": false,
            "array": [1, 2],
            "object": {"a": 1},
        });
        let response_headers =
            header_map(indexmap! {"Token" => "Secret Value"});

        let request = RequestRecord {
            recipe_id: recipe.id.clone(),
            profile_id: Some(profile.id.clone()),
            ..RequestRecord::factory(())
        };
        let response = ResponseRecord {
            body: response_body.to_string().into_bytes().into(),
            headers: response_headers,
            ..ResponseRecord::factory(())
        };
        database
            .insert_exchange(&Exchange::factory((request, response)))
            .unwrap();

        let context = TemplateContext {
            selected_profile: Some(profile.id.clone()),
            collection: Collection {
                recipes: by_id([recipe]).into(),
                chains: by_id([chain]),
                profiles: by_id([profile]),
                ..Collection::factory(())
            }
            .into(),
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
        Chain {
            id: "unknown".into(),
            ..Chain::factory(())
        },
        None,
        None,
        "Unknown chain"
    )]
    // Chain references a recipe that's not in the collection
    #[case::unknown_recipe(
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
            ..Exchange::factory(RecipeId::from("recipe1"))
        }),
        "content type not provided",
    )]
    // Response can't be parsed according to the content type we gave
    #[case::parse_response(
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
            ..Exchange::factory(RecipeId::from("recipe1"))
        }),
        "Parsing response: expected ident at line 1 column 2",
    )]
    // Query returned multiple results
    #[case::query_multiple_results(
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
            ..Exchange::factory(RecipeId::from("recipe1"))
        }),
        "Expected exactly one result",
    )]
    #[tokio::test]
    async fn test_chain_request_error(
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

        let context = TemplateContext {
            collection: Collection {
                recipes: recipes.into(),
                chains: by_id([chain]),
                ..Collection::factory(())
            }
            .into(),
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
        http_engine: &HttpEngine,
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
        let server = MockServer::start().await;
        let host = server.uri();
        Mock::given(matchers::method("GET"))
            .and(matchers::path("/get"))
            .respond_with(ResponseTemplate::new(200).set_body_string("hello!"))
            .mount(&server)
            .await;

        let recipe = Recipe {
            url: format!("{host}/get").into(),
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
        let context = TemplateContext {
            collection: Collection {
                recipes: by_id([recipe]).into(),
                chains: by_id([chain]),
                ..Collection::factory(())
            }
            .into(),
            http_engine: Some(http_engine.clone()),
            database,
            ..TemplateContext::factory(())
        };

        assert_eq!(render!("{{chains.chain1}}", context).unwrap(), "hello!");
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
                chains: by_id([chain]),
                ..Collection::factory(())
            }
            .into(),
            ..TemplateContext::factory(())
        };

        assert_eq!(render!("{{chains.chain1}}", context).unwrap(), expected);
    }

    /// Test failure with chained command
    #[rstest]
    #[case::no_command(&[], None, "No command given")]
    #[case::unknown_command(
        &["totally not a program"], None, if cfg!(unix) {
            "No such file or directory"
        } else {
            "program not found"
        }
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
                chains: by_id([chain]),
                ..Collection::factory(())
            }
            .into(),
            ..TemplateContext::factory(())
        };

        assert_err!(render!("{{chains.chain1}}", context), expected_error);
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
                chains: by_id([chain]),
                ..Collection::factory(())
            }
            .into(),
            ..TemplateContext::factory(())
        };

        assert_eq!(render!("{{chains.chain1}}", context).unwrap(), expected);
    }

    /// Test success with a chained environment variable
    #[rstest]
    #[case::present(Some("test!"), "test!")]
    #[case::missing(None, "")]
    #[tokio::test]
    async fn test_chain_environment(
        #[case] env_value: Option<&str>,
        #[case] expected: &str,
    ) {
        let source = ChainSource::Environment {
            variable: "TEST".into(),
        };
        let chain = Chain {
            source,
            ..Chain::factory(())
        };
        let context = TemplateContext {
            collection: Collection {
                chains: by_id([chain]),
                ..Collection::factory(())
            }
            .into(),
            ..TemplateContext::factory(())
        };
        // This prevents tests from competing for environment variables, and
        // isolates us from the external env
        let result = {
            let _guard = env_lock::lock_env([("TEST", env_value)]);
            render!("{{chains.chain1}}", context)
        };
        assert_eq!(result.unwrap(), expected);
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
                chains: by_id([chain]),
                ..Collection::factory(())
            }
            .into(),
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
                chains: by_id([chain]),
                ..Collection::factory(())
            }
            .into(),
            ..TemplateContext::factory(())
        };

        assert_err!(
            render!("{{chains.chain1}}", context),
            "Reading file `not-real`"
        );
    }

    #[rstest]
    #[case::response(Some("hello!"), "hello!")]
    #[case::default(None, "default")]
    #[tokio::test]
    async fn test_chain_prompt(
        #[case] response: Option<&str>,
        #[case] expected: &str,
    ) {
        let chain = Chain {
            source: ChainSource::Prompt {
                message: Some("password".into()),
                default: Some("default".into()),
            },
            ..Chain::factory(())
        };

        // Test value from prompter
        let context = TemplateContext {
            collection: Collection {
                chains: by_id([chain]),
                ..Collection::factory(())
            }
            .into(),

            prompter: Box::new(TestPrompter::new(response)),
            ..TemplateContext::factory(())
        };
        assert_eq!(render!("{{chains.chain1}}", context).unwrap(), expected);
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
                chains: by_id([chain]),
                ..Collection::factory(())
            }
            .into(),
            // Prompter gives no response
            prompter: Box::<TestPrompter>::default(),
            ..TemplateContext::factory(())
        };

        assert_err!(
            render!("{{chains.chain1}}", context),
            "No response from prompt/select"
        );
    }

    #[rstest]
    #[case::no_chains(vec!["foo!", "bar!"], Some("bar!"), "bar!")]
    #[tokio::test]
    async fn test_chain_select(
        #[case] options: Vec<&str>,
        #[case] response: Option<&str>,
        #[case] expected: &str,
    ) {
        let sut_chain = Chain {
            source: ChainSource::Select {
                message: Some("password".into()),
                options: options.into_iter().map(|s| s.into()).collect(),
            },
            ..Chain::factory(())
        };

        // Test value from prompter
        let context = TemplateContext {
            collection: Collection {
                chains: by_id([sut_chain]),
                ..Collection::factory(())
            }
            .into(),

            prompter: Box::new(TestPrompter::new(response)),
            ..TemplateContext::factory(())
        };
        assert_eq!(render!("{{chains.chain1}}", context).unwrap(), expected);
    }

    #[tokio::test]
    async fn test_chain_select_error() {
        let chain = Chain {
            source: ChainSource::Select {
                message: Some("password".into()),
                options: vec!["foo".into(), "bar".into()],
            },
            ..Chain::factory(())
        };
        let context = TemplateContext {
            collection: Collection {
                chains: by_id([chain]),
                ..Collection::factory(())
            }
            .into(),
            // Prompter gives no response
            prompter: Box::<TestPrompter>::default(),
            ..TemplateContext::factory(())
        };

        assert_err!(
            render!("{{chains.chain1}}", context),
            "No response from prompt/select"
        );
    }

    /// Test that a chain being used twice only computes the chain once
    #[tokio::test]
    async fn test_chain_duplicate() {
        let chain = Chain {
            source: ChainSource::Prompt {
                message: None,
                default: None,
            },
            ..Chain::factory(())
        };

        let context = TemplateContext {
            collection: Collection {
                chains: by_id([chain]),
                ..Collection::factory(())
            }
            .into(),

            prompter: Box::new(TestPrompter::new(["first", "second"])),
            ..TemplateContext::factory(())
        };
        assert_eq!(
            render!("{{chains.chain1}} {{chains.chain1}}", context).unwrap(),
            "first first"
        );
    }

    /// When a chain is used twice and it produces an error, we should see the
    /// error twice in the chunk result, but only once in the consolidated
    /// result
    #[tokio::test]
    async fn test_chain_duplicate_error() {
        let chain = Chain {
            source: ChainSource::Prompt {
                message: None,
                default: None,
            },
            ..Chain::factory(())
        };
        let chain_id = chain.id.clone();
        let context = TemplateContext {
            collection: Collection {
                chains: by_id([chain]),
                ..Collection::factory(())
            }
            .into(),

            prompter: Box::<TestPrompter>::default(),
            ..TemplateContext::factory(())
        };
        let template = Template::from("{{chains.chain1}}{{chains.chain1}}");

        // Chunked render
        let expected_error = TemplateError::Chain {
            chain_id,
            error: ChainError::PromptNoResponse,
        };
        assert_eq!(
            template.render_chunks(&context).await,
            vec![
                TemplateChunk::Error(expected_error.clone()),
                TemplateChunk::Error(expected_error)
            ]
        );

        // Consolidated render
        assert_err!(render!(template, context), "No response from prompt");
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
                chains: by_id([chain]),
                ..Collection::factory(())
            }
            .into(),
            // Prompter gives no response
            prompter: Box::new(TestPrompter::new(["hello!"])),
            ..TemplateContext::factory(())
        };
        assert_eq!(
            Template::from("{{chains.chain1}}")
                .render_chunks(&context)
                .await,
            vec![TemplateChunk::Rendered {
                value: Arc::new("hello!".into()),
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
                chains: by_id([file_chain, command_chain]),
                ..Collection::factory(())
            }
            .into(),
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
                chains: by_id([file_chain, command_chain]),
                ..Collection::factory(())
            }
            .into(),
            ..TemplateContext::factory(())
        };
        let expected = if cfg!(unix) {
            "Rendering nested template for field `command[2]`: \
            Resolving chain `file`: Reading file `bogus.txt`: \
            No such file or directory"
        } else {
            "Rendering nested template for field `command[2]`: \
            Resolving chain `file`: Reading file `bogus.txt`: \
            The system cannot find the file specified. (os error 2)"
        };
        assert_err!(render!("{{chains.command}}", context), expected);
    }

    #[rstest]
    #[case::present(Some("test!"), "test!")]
    #[case::missing(None, "")]
    #[tokio::test]
    async fn test_environment_success(
        #[case] env_value: Option<&str>,
        #[case] expected: &str,
    ) {
        let context = TemplateContext::factory(());
        // This prevents tests from competing for environ environment variables,
        // and isolates us from the external env
        let result = {
            let _guard = env_lock::lock_env([("TEST", env_value)]);
            render!("{{env.TEST}}", context)
        };
        assert_eq!(result.unwrap(), expected);
    }

    /// Test rendering non-UTF-8 data
    #[rstest]
    #[tokio::test]
    async fn test_render_binary(invalid_utf8_chain: ChainSource) {
        let chain = Chain {
            source: invalid_utf8_chain,
            ..Chain::factory(())
        };
        let context = TemplateContext {
            collection: Collection {
                chains: by_id([chain]),
                ..Collection::factory(())
            }
            .into(),
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
    #[rstest]
    #[tokio::test]
    async fn test_render_invalid_utf8(invalid_utf8_chain: ChainSource) {
        let chain = Chain {
            source: invalid_utf8_chain,
            ..Chain::factory(())
        };
        let context = TemplateContext {
            collection: Collection {
                chains: by_id([chain]),
                ..Collection::factory(())
            }
            .into(),
            ..TemplateContext::factory(())
        };

        assert_err!(render!("{{chains.chain1}}", context), "invalid utf-8");
    }

    /// Test rendering into individual chunks with complex unicode
    #[tokio::test]
    async fn test_render_chunks() {
        let context =
            profile_context(indexmap! { "user_id".into() => "🧡💛".into() });

        let chunks =
            Template::from("intro {{user_id}} 💚💙💜 {{unknown}} outro")
                .render_chunks(&context)
                .await;
        assert_eq!(
            chunks,
            vec![
                TemplateChunk::raw("intro "),
                TemplateChunk::Rendered {
                    value: Arc::new("🧡💛".into()),
                    sensitive: false
                },
                // Each emoji is 4 bytes
                TemplateChunk::raw(" 💚💙💜 "),
                TemplateChunk::Error(TemplateError::FieldUnknown {
                    field: "unknown".into()
                }),
                TemplateChunk::raw(" outro"),
            ]
        );
    }

    /// Tested rendering a template with escaped keys, which should be treated
    /// as raw text
    #[tokio::test]
    async fn test_render_escaped() {
        let context =
            profile_context(indexmap! { "user_id".into() => "user1".into() });
        let template = "user: {{user_id}} escaped: {_{user_id}}";
        assert_eq!(
            render!(template, context).unwrap(),
            "user: user1 escaped: {{user_id}}"
        );
    }

    /// Build a template context that only has simple profile data
    fn profile_context(data: IndexMap<String, Template>) -> TemplateContext {
        let profile = Profile {
            data,
            ..Profile::factory(())
        };
        let profile_id = profile.id.clone();
        TemplateContext {
            collection: Collection {
                profiles: by_id([profile]),
                ..Collection::factory(())
            }
            .into(),
            selected_profile: Some(profile_id),
            ..TemplateContext::factory(())
        }
    }

    /// Test various cases that should trigger cycle detection
    #[rstest]
    #[case::field("{{infinite}}")]
    #[case::chain("{{chains.infinite}}")]
    #[case::chain_second("{{chains.ok}} {{chains.infinite}}")]
    #[case::mutual_field("{{mutual1}}")]
    #[case::mutual_chain("{{chains.mutual1}}")]
    #[tokio::test]
    async fn test_infinite_loops(#[case] template: Template) {
        let profile = Profile {
            data: indexmap! {
                "infinite".into() => "{{infinite}}".into(),
                "mutual1".into() => "{{mutual2}}".into(),
                "mutual2".into() => "{{mutual1}}".into(),
            },
            ..Profile::factory(())
        };
        let profile_id = profile.id.clone();

        let chains = [
            Chain {
                id: "ok".into(),
                source: ChainSource::command(["echo"]),
                ..Chain::factory(())
            },
            Chain {
                id: "infinite".into(),
                source: ChainSource::command(["echo", "{{chains.infinite}}"]),
                ..Chain::factory(())
            },
            Chain {
                id: "mutual1".into(),
                source: ChainSource::command(["echo", "{{chains.mutual2}}"]),
                ..Chain::factory(())
            },
            Chain {
                id: "mutual2".into(),
                source: ChainSource::command(["echo", "{{chains.mutual1}}"]),
                ..Chain::factory(())
            },
        ];

        let context = TemplateContext {
            collection: Collection {
                profiles: by_id([profile]),
                chains: by_id(chains),
                ..Collection::factory(())
            }
            .into(),
            selected_profile: Some(profile_id),
            ..TemplateContext::factory(())
        };

        assert_err!(
            render!(template, context),
            "Infinite loop detected in template"
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
