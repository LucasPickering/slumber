use crate::{
    collection::{
        self, Chain, ChainOutputTrim, ChainSource, Collection, Folder, Profile,
        ProfileId, Recipe, RecipeId, RecipeNode, RecipeTree,
    },
    config::Config,
    db::CollectionDatabase,
    http::{Body, Request, RequestId, RequestRecord, Response},
    template::{Prompt, Prompter, Template, TemplateContext},
    tui::{
        context::TuiContext,
        message::{Message, MessageSender},
    },
    util::ResultExt,
};
use anyhow::Context;
use chrono::Utc;
use derive_more::Deref;
use indexmap::{indexmap, IndexMap};
use ratatui::{backend::TestBackend, Terminal};
use reqwest::{
    header::{HeaderMap, HeaderName, HeaderValue},
    StatusCode,
};
use std::{
    env, fs,
    path::{Path, PathBuf},
};
use tokio::sync::{mpsc, mpsc::UnboundedReceiver};
use uuid::Uuid;

/// Test-only trait to build a placeholder instance of a struct. This is similar
/// to `Default`, but allows for useful placeholders that may not make sense in
/// the context of the broader app. It also makes it possible to implement a
/// factory for a type that already has `Default`.
///
/// Factories can also be parameterized, meaning the implementor can define
/// convenient knobs to let the caller customize the generated type. Each type
/// can have any number of `Factory` implementations, so you can support
/// multiple param types.
pub trait Factory<Param = ()> {
    fn factory(param: Param) -> Self;
}

impl Factory for Collection {
    fn factory(_: ()) -> Self {
        let recipe = Recipe::factory(());
        let profile = Profile::factory(());
        Collection {
            recipes: indexmap! {recipe.id.clone() => recipe}.into(),
            profiles: indexmap! {profile.id.clone() => profile},
            ..Collection::default()
        }
    }
}

impl Factory for ProfileId {
    fn factory(_: ()) -> Self {
        Uuid::new_v4().to_string().into()
    }
}

impl Factory for RecipeId {
    fn factory(_: ()) -> Self {
        Uuid::new_v4().to_string().into()
    }
}

impl Factory for Profile {
    fn factory(_: ()) -> Self {
        Self {
            id: "profile1".into(),
            name: None,
            data: IndexMap::new(),
        }
    }
}

impl Factory for Folder {
    fn factory(_: ()) -> Self {
        Self {
            id: "folder1".into(),
            name: None,
            children: IndexMap::new(),
        }
    }
}

impl Factory for Recipe {
    fn factory(_: ()) -> Self {
        Self {
            id: "recipe1".into(),
            name: None,
            method: collection::Method::Get,
            url: "http://localhost/url".into(),
            body: None,
            authentication: None,
            query: IndexMap::new(),
            headers: IndexMap::new(),
        }
    }
}

impl Factory for Chain {
    fn factory(_: ()) -> Self {
        Self {
            id: "chain1".into(),
            source: ChainSource::Request {
                recipe: "recipe1".into(),
                trigger: Default::default(),
                section: Default::default(),
            },
            sensitive: false,
            selector: None,
            content_type: None,
            trim: ChainOutputTrim::default(),
        }
    }
}

impl Factory for Request {
    fn factory(_: ()) -> Self {
        Self {
            id: RequestId::new(),
            profile_id: None,
            recipe_id: "recipe1".into(),
            method: reqwest::Method::GET,
            url: "http://localhost/url".parse().unwrap(),
            headers: HeaderMap::new(),
            body: None,
        }
    }
}

/// Customize profile and recipe ID
impl Factory<(Option<ProfileId>, RecipeId)> for Request {
    fn factory((profile_id, recipe_id): (Option<ProfileId>, RecipeId)) -> Self {
        Self {
            id: RequestId::new(),
            profile_id,
            recipe_id,
            method: reqwest::Method::GET,
            url: "http://localhost/url".parse().unwrap(),
            headers: HeaderMap::new(),
            body: None,
        }
    }
}

impl Factory for Response {
    fn factory(_: ()) -> Self {
        Self {
            status: StatusCode::OK,
            headers: HeaderMap::new(),
            body: Body::default(),
        }
    }
}

impl Factory for RequestRecord {
    fn factory(_: ()) -> Self {
        let request = Request::factory(());
        let response = Response::factory(());
        Self {
            id: request.id,
            request: request.into(),
            response: response.into(),
            start_time: Utc::now(),
            end_time: Utc::now(),
        }
    }
}

/// Customize profile and recipe ID
impl Factory<(Option<ProfileId>, RecipeId)> for RequestRecord {
    fn factory(params: (Option<ProfileId>, RecipeId)) -> Self {
        let request = Request::factory(params);
        let response = Response::factory(());
        Self {
            id: request.id,
            request: request.into(),
            response: response.into(),
            start_time: Utc::now(),
            end_time: Utc::now(),
        }
    }
}

impl Factory for TemplateContext {
    fn factory(_: ()) -> Self {
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

/// Directory containing static test data
#[fixture]
pub fn test_data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("test_data")
}

/// Create a terminal instance for testing
#[fixture]
pub fn terminal(
    terminal_width: u16,
    terminal_height: u16,
) -> Terminal<TestBackend> {
    let backend = TestBackend::new(terminal_width, terminal_height);
    Terminal::new(backend).unwrap()
}

/// For injection to [terminal] fixture
#[fixture]
fn terminal_width() -> u16 {
    40
}

/// For injection to [terminal] fixture
#[fixture]
fn terminal_height() -> u16 {
    20
}

/// Create an in-memory database for a collection
#[fixture]
pub fn database() -> CollectionDatabase {
    CollectionDatabase::factory(())
}

/// Test fixture for using TUI context. The context is a global read-only var,
/// so this will initialize it once for *all tests*.
#[fixture]
#[once]
pub fn tui_context() -> &'static TuiContext {
    TuiContext::init(Config::default());
    TuiContext::get()
}

/// Create a new temporary folder. This will include a random subfolder to
/// guarantee uniqueness for this test.
#[fixture]
pub fn temp_dir() -> TempDir {
    TempDir::new()
}

/// Guard for a temporary directory. Create the directory on creation, delete
/// it on drop.
#[derive(Debug, Deref)]
pub struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let path = env::temp_dir().join(Uuid::new_v4().to_string());
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        // Clean up
        let _ = fs::remove_dir_all(&self.0)
            .with_context(|| {
                format!("Error deleting temporary directory {:?}", self.0)
            })
            .traced();
    }
}

#[fixture]
pub fn messages() -> MessageQueue {
    let (tx, rx) = mpsc::unbounded_channel();
    MessageQueue { tx: tx.into(), rx }
}

/// Test-only wrapper for MPSC receiver, to test what messages have been queued
pub struct MessageQueue {
    tx: MessageSender,
    rx: UnboundedReceiver<Message>,
}

impl MessageQueue {
    /// Get the message sender
    pub fn tx(&self) -> &MessageSender {
        &self.tx
    }

    pub fn assert_empty(&mut self) {
        let message = self.rx.try_recv().ok();
        assert!(
            message.is_none(),
            "Expected empty queue, but had message {message:?}"
        );
    }

    /// Pop the next message off the queue. Panic if the queue is empty
    pub fn pop_now(&mut self) -> Message {
        self.rx.try_recv().expect("Message queue empty")
    }

    /// Pop the next message off the queue, waiting if empty
    pub async fn pop_wait(&mut self) -> Message {
        self.rx.recv().await.expect("Message queue closed")
    }

    /// Clear all messages in the queue
    pub fn clear(&mut self) {
        while self.rx.try_recv().is_ok() {}
    }
}

/// Return a static value when prompted, or no value if none is given
#[derive(Debug, Default)]
pub struct TestPrompter {
    value: Option<String>,
}

impl TestPrompter {
    pub fn new<T: Into<String>>(value: Option<T>) -> Self {
        Self {
            value: value.map(Into::into),
        }
    }
}

impl Prompter for TestPrompter {
    fn prompt(&self, prompt: Prompt) {
        // If no value was given, check default. If no default, don't respond
        if let Some(value) = self.value.as_ref() {
            prompt.channel.respond(value.clone())
        } else if let Some(default) = prompt.default {
            prompt.channel.respond(default);
        }
    }
}

// Some helpful conversion implementations
impl From<&str> for ProfileId {
    fn from(value: &str) -> Self {
        value.to_owned().into()
    }
}

impl From<IndexMap<RecipeId, Recipe>> for RecipeTree {
    fn from(value: IndexMap<RecipeId, Recipe>) -> Self {
        let tree = value
            .into_iter()
            .map(|(id, recipe)| (id, RecipeNode::Recipe(recipe)))
            .collect();
        Self::new(tree).expect("Duplicate recipe ID")
    }
}

impl From<&str> for RecipeId {
    fn from(value: &str) -> Self {
        value.to_owned().into()
    }
}

impl From<&str> for Template {
    fn from(value: &str) -> Self {
        value.to_owned().try_into().unwrap()
    }
}
// Can't implement this for From<String> because it conflicts with TryFrom

/// Helper for creating a header map
pub fn header_map<'a>(
    headers: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> HeaderMap {
    headers
        .into_iter()
        .map(|(header, value)| {
            (
                HeaderName::try_from(header).unwrap(),
                HeaderValue::try_from(value).unwrap(),
            )
        })
        .collect()
}

/// Assert a result is the `Err` variant, and the stringified error contains
/// the given message
macro_rules! assert_err {
    ($e:expr, $msg:expr) => {{
        use itertools::Itertools as _;

        let msg = $msg;
        // Include all source errors so wrappers don't hide the important stuff
        let error: anyhow::Error = $e.unwrap_err().into();
        let actual = error.chain().map(ToString::to_string).join(": ");
        assert!(
            actual.contains(msg),
            "Expected error message to contain {msg:?}, but was: {actual:?}"
        )
    }};
}
pub(crate) use assert_err;

/// Assert the given expression matches a pattern. Optionally extract bound
/// values from the pattern using the `=>` syntax.
macro_rules! assert_matches {
    ($expr:expr, $pattern:pat $(,)?) => {
        assert_matches!($expr, $pattern => ());
    };
    ($expr:expr, $pattern:pat => $bindings:expr $(,)?) => {
        match $expr {
            $pattern => $bindings,
            value => panic!(
                "Unexpected value; \
                {value:?} does not match expected {expected}",
                expected = stringify!($pattern)
            ),
        }
    };
}
pub(crate) use assert_matches;

/// Assert that the event queue matches the given list of patterns
macro_rules! assert_events {
    ($($pattern:pat),* $(,)?) => {
        ViewContext::inspect_event_queue(|events| {
            assert_matches!(events, &[$($pattern,)*]);
        });
    }
}
pub(crate) use assert_events;
use rstest::fixture;
