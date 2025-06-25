use crate::{
    collection::{
        Chain, ChainOutputTrim, ChainRequestSection, ChainRequestTrigger,
        ChainSource, Collection, Profile, Recipe, RecipeId, SelectOptions,
    },
    database::CollectionDatabase,
    http::{
        Exchange, HttpEngine, RequestRecord, ResponseRecord,
        content_type::ContentType,
    },
    template::{
        ChainError, RenderError, Template, TemplateChunk, TemplateContext,
    },
    test_util::{
        TestHttpProvider, TestPrompter, TestSelectPrompter, by_id, header_map,
        http_engine, invalid_utf8_chain,
    },
};
use chrono::Utc;
use indexmap::{IndexMap, indexmap};
use rstest::rstest;
use serde_json::json;
use slumber_util::{Factory, TempDir, assert_err, temp_dir};
use std::time::Duration;
use tokio::fs;
use wiremock::{Mock, MockServer, ResponseTemplate, matchers};

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
        render!("{{ field1 }}", context).unwrap(),
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
        "start {{ user_id }} 🧡💛 {{group_id}} end",
        "start 1 🧡💛 3 end"
    )]
#[tokio::test]
async fn test_field(#[case] template: &str, #[case] expected: &str) {
    let context = profile_context(indexmap! {
        "user_id".into() => "1".into(),
        "group_id".into() => "3".into(),
        "nested".into() => "user id: {{ user_id }}".into(),
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
    use crate::database::CollectionDatabase;

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
        "array": [1, 2],
        "bool": false,
        "number": 6,
        "object": {"a": 1},
        "string": "Hello World!",
    });
    let response_headers = header_map(indexmap! {"Token" => "Secret Value"});

    let request = RequestRecord {
        recipe_id: recipe.id.clone(),
        profile_id: Some(profile.id.clone()),
        ..RequestRecord::factory(())
    };
    let response = ResponseRecord {
        id: request.id,
        body: response_body.into(),
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
        }
        .into(),
        http_provider: Box::new(TestHttpProvider::new(database, None)),
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
// Query returned no results
#[case::query_multiple_results(
        Chain {
            source: ChainSource::Request {
                recipe: "recipe1".into(),
                trigger: Default::default(),
                section:Default::default()
            },
            selector: Some("$.bogus".parse().unwrap()),
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
        "No results from JSONPath query",
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
    use indexmap::IndexMap;

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
        http_provider: Box::new(TestHttpProvider::new(database, None)),
        ..TemplateContext::factory(())
    };

    assert_err!(render!("{{chains.chain1}}", context), expected_error);
}

/// Test triggered sub-requests. We expect all of these *to trigger*
#[rstest]
#[case::no_history(ChainRequestTrigger::NoHistory, None)]
#[case::expire_empty(ChainRequestTrigger::Expire(Duration::from_secs(0)), None)]
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
    http_engine: HttpEngine,
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
        http_provider: Box::new(TestHttpProvider::new(
            database,
            Some(http_engine.clone()),
        )),
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
#[case::no_chains(
        SelectOptions::Fixed(vec!["foo!".into(), "bar!".into()]), 0, "foo!",
    )]
#[case::chains_first(
        SelectOptions::Fixed(vec!["foo!".into(), "{{chains.command}}".into()]),
        0,
        "foo!",
    )]
#[case::chains_second(
        SelectOptions::Fixed(vec!["foo!".into(), "{{chains.command}}".into()]),
        1,
        "command_output",
    )]
#[tokio::test]
async fn test_chain_fixed_select(
    #[case] options: SelectOptions,
    #[case] index: usize,
    #[case] expected: &str,
) {
    let sut_chain = Chain {
        id: "sut".into(),
        source: ChainSource::Select {
            message: Some("password".into()),
            options,
        },
        ..Chain::factory(())
    };

    let command_chain = Chain {
        id: "command".into(),
        source: ChainSource::command(["echo", "command_output"]),
        trim: ChainOutputTrim::Both,
        ..Chain::factory(())
    };

    let context = TemplateContext {
        collection: Collection {
            chains: by_id([sut_chain, command_chain]),
            ..Collection::factory(())
        }
        .into(),
        prompter: Box::new(TestSelectPrompter::new(vec![index])),
        ..TemplateContext::factory(())
    };

    assert_eq!(render!("{{chains.sut}}", context).unwrap(), expected);
}

#[rstest]
#[case::dynamic_select_first(0, "foo")]
#[case::dynamic_select_second(1, "bar")]
#[tokio::test]
async fn test_chain_dynamic_select(
    #[case] index: usize,
    #[case] expected: &str,
) {
    let profile = Profile {
        data: indexmap! {"header".into() => "Token".into()},
        ..Profile::factory(())
    };
    let recipe = Recipe {
        ..Recipe::factory(())
    };

    let sut_chain = Chain {
        id: "sut".into(),
        source: ChainSource::Select {
            message: Some("password".into()),
            options: SelectOptions::Dynamic("{{chains.request}}".into()),
        },
        ..Chain::factory(())
    };

    let request_chain = Chain {
        id: "request".into(),
        source: ChainSource::Request {
            recipe: recipe.id.clone(),
            trigger: Default::default(),
            section: Default::default(),
        },
        selector: None,
        content_type: Some(ContentType::Json),
        ..Chain::factory(())
    };

    let database = CollectionDatabase::factory(());

    let response_headers = header_map(indexmap! {"Token" => "Secret Value"});

    let request = RequestRecord {
        recipe_id: recipe.id.clone(),
        profile_id: Some(profile.id.clone()),
        ..RequestRecord::factory(())
    };
    let response = ResponseRecord {
        id: request.id,
        body: json!(["foo", "bar"]).into(),
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
            chains: by_id([sut_chain, request_chain]),
            profiles: by_id([profile]),
        }
        .into(),
        http_provider: Box::new(TestHttpProvider::new(database, None)),
        prompter: Box::new(TestSelectPrompter::new(vec![index])),
        ..TemplateContext::factory(())
    };

    assert_eq!(render!("{{chains.sut}}", context).unwrap(), expected);
}

#[tokio::test]
async fn test_chain_select_error() {
    let chain = Chain {
        source: ChainSource::Select {
            message: Some("password".into()),
            options: SelectOptions::Fixed(vec!["foo".into(), "bar".into()]),
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
        prompter: Box::<TestSelectPrompter>::default(),
        ..TemplateContext::factory(())
    };

    assert_err!(
        render!("{{chains.chain1}}", context),
        "No response from prompt/select"
    );
}

#[rstest]
#[case::json_string(
    "not json",
    "Dynamic option list failed to deserialize as JSON"
)]
#[case::json_object(
    "{\"a\": 3}",
    "Dynamic option list failed to deserialize as JSON: \
        invalid type: map, expected a sequence"
)]
#[tokio::test]
async fn test_chain_select_dynamic_error(
    #[case] input: &str,
    #[case] expected_error: &str,
) {
    let sut_chain = Chain {
        source: ChainSource::Select {
            message: Some("password".into()),
            options: SelectOptions::Dynamic("{{chains.command}}".into()),
        },
        ..Chain::factory(())
    };

    let command_chain = Chain {
        id: "command".into(),
        source: ChainSource::command(["echo", input]),
        ..Chain::factory(())
    };

    let context = TemplateContext {
        collection: Collection {
            chains: by_id([sut_chain, command_chain]),
            ..Collection::factory(())
        }
        .into(),
        prompter: Box::new(TestSelectPrompter::new(vec![0usize])),
        ..TemplateContext::factory(())
    };

    assert_err!(render!("{{chains.chain1}}", context), expected_error);
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
    let expected_error = RenderError::Chain {
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
        source: ChainSource::command(["echo", "-n", "answer: {{chains.file}}"]),
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
        source: ChainSource::command(["echo", "-n", "answer: {{chains.file}}"]),
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

    let chunks = Template::from("intro {{ user_id }} 💚💙💜 {{unknown}} outro")
        .render_chunks(&context)
        .await;
    assert_eq!(
        chunks,
        vec![
            TemplateChunk::raw("intro "),
            TemplateChunk::Rendered {
                value: "🧡💛".into(),
                sensitive: false
            },
            // Each emoji is 4 bytes
            TemplateChunk::raw(" 💚💙💜 "),
            TemplateChunk::Error(RenderError::FieldUnknown {
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
    let template = "user: {{ user_id }} escaped: {_{user_id}}";
    assert_eq!(
        render!(template, context).unwrap(),
        "user: user1 escaped: {{ user_id }}"
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
