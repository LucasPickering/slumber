use crate::{
    collection::{
        Chain, ChainSource, Collection, Folder, Profile, Recipe, RecipeId,
        RecipeNode, RecipeTree,
    },
    config::Config,
    db::CollectionDatabase,
    http::{Body, Request, RequestId, RequestRecord, Response},
    template::{Prompt, Prompter, Template, TemplateContext},
    tui::{
        context::TuiContext,
        message::{Message, MessageSender},
    },
};
use ratatui::{backend::TestBackend, Terminal};
use std::{
    env, fs,
    path::{Path, PathBuf},
};
use tokio::sync::{mpsc, mpsc::UnboundedReceiver};
use uuid::Uuid;

use chrono::Utc;
use factori::{create, factori};
use indexmap::IndexMap;
use reqwest::{
    header::{HeaderMap, HeaderName, HeaderValue},
    Method, StatusCode,
};

factori!(Collection, {
    default {
        profiles = Default::default(),
        chains = Default::default(),
        recipes = Default::default(),
        _ignore = Default::default(),
    }
});

factori!(Profile, {
    default {
        id = "profile1".into(),
        name = None,
        data = Default::default(),
    }
});

factori!(Folder, {
    default {
        id = "folder1".into(),
        name = None,
        children = Default::default(),
    }
});

factori!(Recipe, {
    default {
        id = "recipe1".into(),
        name = None,
        method = "GET".parse().unwrap(),
        url = "http://localhost".into(),
        body = None,
        authentication = None,
        query = Default::default(),
        headers = Default::default(),
    }
});

factori!(Request, {
    default {
        id = RequestId::new(),
        profile_id = None,
        recipe_id = "recipe1".into(),
        method = Method::GET,
        url = "http://localhost/url".parse().unwrap(),
        headers = HeaderMap::new(),
        body = None,
    }
});

factori!(Response, {
    default {
        status = StatusCode::OK,
        headers = HeaderMap::new(),
        body = Body::default(),
    }
});

// Apparently you can't use a macro in the factori init expression so we have
// to hide them behind functions
fn request() -> Request {
    create!(Request)
}
fn response() -> Response {
    create!(Response)
}

factori!(RequestRecord, {
    default {
        id = RequestId::new(),
        request = request().into(),
        response = response().into(),
        start_time = Utc::now(),
        end_time = Utc::now(),
    }
});

factori!(Chain, {
    default {
        id = "chain1".into(),
        source = ChainSource::Request {
            recipe: RecipeId::default(),
            trigger: Default::default(),
            section: Default::default(),
        },
        sensitive = false,
        selector = None,
        content_type = None,
    }
});

factori!(TemplateContext, {
    default {
        selected_profile = None,
        collection = Default::default(),
        prompter = Box::<TestPrompter>::default(),
        http_engine = None,
        database = CollectionDatabase::testing(),
        overrides = Default::default(),
        recursion_count = Default::default(),
    }
});

/// Directory containing static test data
#[rstest::fixture]
pub fn test_data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("test_data")
}

/// Create a new temporary folder. This will include a random subfolder to
/// guarantee uniqueness for this test.
#[rstest::fixture]
pub fn temp_dir() -> PathBuf {
    let path = env::temp_dir().join(Uuid::new_v4().to_string());
    fs::create_dir(&path).unwrap();
    path
}

/// Create a terminal instance for testing
#[rstest::fixture]
pub fn terminal() -> Terminal<TestBackend> {
    let backend = TestBackend::new(10, 10);
    Terminal::new(backend).unwrap()
}

/// Test fixture for using context. This will initialize it once for all tests
#[rstest::fixture]
#[once]
pub fn tui_context() {
    TuiContext::init(Config::default(), CollectionDatabase::testing());
}

#[rstest::fixture]
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
impl From<IndexMap<RecipeId, Recipe>> for RecipeTree {
    fn from(value: IndexMap<RecipeId, Recipe>) -> Self {
        let tree = value
            .into_iter()
            .map(|(id, recipe)| (id, RecipeNode::Recipe(recipe)))
            .collect();
        Self::new(tree).expect("Duplicate recipe ID")
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
