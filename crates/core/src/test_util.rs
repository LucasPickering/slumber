//! General test utilities, that apply to all parts of the program

use crate::{
    collection::{HasId, ProfileId, RecipeId},
    database::CollectionDatabase,
    http::{
        Exchange, HttpEngine, RequestSeed, StoredRequestError,
        TriggeredRequestError,
    },
    render::{HttpProvider, Prompt, Prompter, TemplateContext},
};
use async_trait::async_trait;
use indexmap::IndexMap;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use rstest::fixture;
use slumber_config::HttpEngineConfig;
use slumber_template::Template;
use std::{
    hash::Hash,
    sync::atomic::{AtomicUsize, Ordering},
};

/// A template that spits out bytes that are *not* valid UTF-8
pub fn invalid_utf8() -> Template {
    "{{ b'\\xc3\\x28' }}".into()
}

/// Create an HTTP engine for building/sending requests. We need to create a new
/// engine for each test because each reqwest client is bound to a specific
/// tokio runtime, and each test gets its own runtime.
/// See <https://github.com/LucasPickering/slumber/pull/524>
#[fixture]
pub fn http_engine() -> HttpEngine {
    HttpEngine::new(&HttpEngineConfig {
        ignore_certificate_hosts: vec!["danger".to_owned()],
        ..Default::default()
    })
}

/// [HttpProvider] implementation for tests. This pulls persisted requests from
/// the DB, but does not persist new requests. Triggered requests are sent only
/// if an HTTP engine is provided.
#[derive(Debug)]
pub struct TestHttpProvider {
    database: CollectionDatabase,
    http_engine: Option<HttpEngine>,
}

impl TestHttpProvider {
    pub fn new(
        database: CollectionDatabase,
        http_engine: Option<HttpEngine>,
    ) -> Self {
        Self {
            database,
            http_engine,
        }
    }
}

#[async_trait(?Send)]
impl HttpProvider for TestHttpProvider {
    async fn get_latest_request(
        &self,
        profile_id: Option<&ProfileId>,
        recipe_id: &RecipeId,
    ) -> Result<Option<Exchange>, StoredRequestError> {
        self.database
            .get_latest_request(profile_id.into(), recipe_id)
            .map_err(StoredRequestError::new)
    }

    async fn send_request(
        &self,
        seed: RequestSeed,
        template_context: &TemplateContext,
    ) -> Result<Exchange, TriggeredRequestError> {
        if let Some(http_engine) = &self.http_engine {
            let ticket = http_engine.build(seed, template_context).await?;
            let exchange = ticket.send().await?;
            Ok(exchange)
        } else {
            Err(TriggeredRequestError::NotAllowed)
        }
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
        match prompt {
            Prompt::Text {
                default, channel, ..
            } => {
                // Grab the next value in the sequence. If we're all out, don't
                // respond
                let index = self.index.fetch_add(1, Ordering::Relaxed);
                if let Some(value) = self.responses.get(index) {
                    channel.reply(value.clone());
                } else if let Some(default) = default {
                    channel.reply(default);
                }
            }
            Prompt::Select { .. } => {
                unimplemented!("TestPrompter does not support selects")
            }
        }
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
    fn prompt(&self, prompt: Prompt) {
        match prompt {
            Prompt::Text { .. } => unimplemented!(
                "TestSelectPrompter does not support text prompts"
            ),
            Prompt::Select {
                mut options,
                channel,
                ..
            } => {
                let index = self.index.fetch_add(1, Ordering::Relaxed);
                if let Some(value_index) = self.responses.get(index) {
                    channel.reply(options.swap_remove(*value_index).value);
                }
            }
        }
    }
}

/// Construct a map of values keyed by their ID
pub fn by_id<T>(values: impl IntoIterator<Item = T>) -> IndexMap<T::Id, T>
where
    T: HasId,
    T::Id: Clone + Eq + Hash,
{
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
