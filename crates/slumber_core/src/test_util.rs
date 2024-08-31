//! General test utilities, that apply to all parts of the program

use crate::{
    collection::{ChainSource, HasId},
    http::{HttpEngine, HttpEngineConfig},
    template::{Prompt, Prompter, Select},
    util::{get_repo_root, ResultTraced},
};
use anyhow::Context;
use derive_more::Deref;
use indexmap::IndexMap;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use rstest::fixture;
use std::{
    env, fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};
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

/// Directory containing static test data
#[fixture]
pub fn test_data_dir() -> PathBuf {
    get_repo_root().join("test_data")
}

/// A chain that spits out bytes that are *not* valid UTF-8
#[fixture]
pub fn invalid_utf8_chain(test_data_dir: PathBuf) -> ChainSource {
    ChainSource::File {
        path: test_data_dir
            .join("invalid_utf8.bin")
            .to_string_lossy()
            .to_string()
            .into(),
    }
}

/// Create a new temporary folder. This will include a random subfolder to
/// guarantee uniqueness for this test.
#[fixture]
pub fn temp_dir() -> TempDir {
    TempDir::new()
}

/// Create an HTTP engine for building/sending requests. This is a singleton
/// because creation is expensive (~300ms), and the engine is immutable.
#[fixture]
#[once]
pub fn http_engine() -> HttpEngine {
    HttpEngine::new(&HttpEngineConfig {
        ignore_certificate_hosts: vec!["danger".to_owned()],
        ..Default::default()
    })
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

/// Response to prompts with zero or more values in sequence
#[derive(Debug, Default)]
pub struct TestPrompter {
    responses: Vec<String>,
    /// Track where in the sequence of responses we are
    index: AtomicUsize,
}

impl TestPrompter {
    pub fn new<T: Into<String>>(
        responses: impl IntoIterator<Item = T>,
    ) -> Self {
        Self {
            responses: responses.into_iter().map(T::into).collect(),
            index: 0.into(),
        }
    }
}

impl Prompter for TestPrompter {
    fn prompt(&self, prompt: Prompt) {
        // Grab the next value in the sequence. If we're all out, don't respond
        let index = self.index.fetch_add(1, Ordering::Relaxed);
        if let Some(value) = self.responses.get(index) {
            prompt.channel.respond(value.clone())
        } else if let Some(default) = prompt.default {
            prompt.channel.respond(default);
        }
    }

    fn select(&self, select: Select) {
        let index = self.index.fetch_add(1, Ordering::Relaxed);
        if let Some(value) = self.responses.get(index) {
            select.channel.respond(value.clone())
        }
    }
}

/// Construct a map of values keyed by their ID
pub fn by_id<T: HasId>(
    values: impl IntoIterator<Item = T>,
) -> IndexMap<T::Id, T> {
    values
        .into_iter()
        .map(|value| (value.id().clone(), value))
        .collect()
}

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
#[macro_export]
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

/// Assert the given expression matches a pattern and optional condition.
/// Additionally, evaluate an expression using the bound pattern. This can be
/// used to apply additional assertions inline, or extract bound values to use
/// in subsequent statements.
#[macro_export]
macro_rules! assert_matches {
    ($expr:expr, $pattern:pat $(if $condition:expr)? $(,)?) => {
        $crate::assert_matches!($expr, $pattern $(if $condition)? => ());
    };
    ($expr:expr, $pattern:pat $(if $condition:expr)? => $output:expr $(,)?) => {
        match $expr {
            // If a conditional was given, check it. This has to be a separate
            // arm to prevent borrow fighting over the matched value
            $(value @ $pattern if !$condition => {
                panic!(
                    "Value {value:?} does not match condition {condition}",
                    condition = stringify!($condition),
                );
            })?
            #[allow(unused_variables)]
            $pattern => $output,
            value => panic!(
                "Unexpected value {value:?} does not match pattern {expected}",
                expected = stringify!($pattern),
            ),
        }
    };
}
