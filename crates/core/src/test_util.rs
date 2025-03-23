//! General test utilities, that apply to all parts of the program

use crate::{
    collection::{ChainSource, HasId},
    http::HttpEngine,
    template::{Prompt, Prompter, Select},
};
use anyhow::Context;
use derive_more::Deref;
use indexmap::IndexMap;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use rstest::fixture;
use slumber_config::HttpEngineConfig;
use slumber_util::{ResultTraced, paths::get_repo_root};
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
pub struct TempDir(#[deref(forward)] PathBuf);

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

    fn select(&self, _select: Select) {
        unimplemented!("TestPrompter does not support selects")
    }
}

/// Response to selects with zero or more values in sequence
#[derive(Debug, Default)]
pub struct TestSelectPrompter {
    /// Index within the contained select to grab response for
    responses: Vec<usize>,
    /// Track where in the sequence of responses we are
    index: AtomicUsize,
}

impl TestSelectPrompter {
    pub fn new(responses: impl IntoIterator<Item = usize>) -> Self {
        Self {
            responses: responses.into_iter().collect(),
            index: 0.into(),
        }
    }
}

impl Prompter for TestSelectPrompter {
    fn prompt(&self, _prompt: Prompt) {
        unimplemented!("TestSelectPrompter does not support prompts")
    }

    fn select(&self, mut select: Select) {
        let index = self.index.fetch_add(1, Ordering::Relaxed);
        if let Some(value) = self.responses.get(index) {
            select.channel.respond(select.options.swap_remove(*value))
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
