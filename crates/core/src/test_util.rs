//! General test utilities, that apply to all parts of the program

use crate::{
    collection::{ChainSource, HasId},
    http::HttpEngine,
    template::{Prompt, Prompter, Select},
};
use indexmap::IndexMap;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use rstest::fixture;
use slumber_config::HttpEngineConfig;
use slumber_util::test_data_dir;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

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
