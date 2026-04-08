//! Utilities for building and sending HTTP requests

use anyhow::Context as _;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use dialoguer::{Input, Password, Select as DialoguerSelect};
use serde::{Deserialize, Serialize};
use slumber_core::{
    collection::{ProfileId, RecipeId},
    database::CollectionDatabase,
    http::{
        Exchange, ExchangeSummary, HttpEngine, RequestId, RequestSeed,
        StoredRequestError, TriggeredRequestError,
    },
    render::{HttpProvider, Prompter, SelectOption, TemplateContext},
};
use slumber_template::Value;
use slumber_util::ResultTracedAnyhow;
use tracing::warn;

/// TODO dedupe w/ TUI
#[derive(Debug, Serialize, Deserialize)]
pub enum RequestState {
    Building {
        id: RequestId,
        start_time: DateTime<Utc>,
    },
    BuildError {
        id: RequestId,
        start_time: DateTime<Utc>,
        end_time: DateTime<Utc>,
        message: String,
    },
    Loading {
        id: RequestId,
        start_time: DateTime<Utc>,
    },
    Response(ExchangeSummary),
    RequestError {
        id: RequestId,
        start_time: DateTime<Utc>,
        end_time: DateTime<Utc>,
        message: String,
    },
}

/// TODO
#[derive(Debug)]
pub struct FilesystemHttpProvider {
    database: CollectionDatabase,
    http_engine: HttpEngine,
}

impl FilesystemHttpProvider {
    pub fn new(database: CollectionDatabase, http_engine: HttpEngine) -> Self {
        Self {
            database,
            http_engine,
        }
    }
}

#[async_trait(?Send)]
impl HttpProvider for FilesystemHttpProvider {
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
        let ticket = self.http_engine.build(seed, template_context).await?;
        let exchange = ticket.send(Some(self.database.clone())).await?;
        Ok(exchange)
    }
}

#[derive(Debug)]
pub struct FilesystemPrompter;

#[async_trait(?Send)]
impl Prompter for FilesystemPrompter {
    async fn prompt_text(
        &self,
        message: String,
        default: Option<String>,
        sensitive: bool,
    ) -> Option<String> {
        // This will implicitly queue the prompts by blocking the main thread.
        // Since the CLI has nothing else to do while waiting on a response,
        // that's fine.
        if sensitive {
            // Dialoguer doesn't support default values here so there's nothing
            // we can do
            if default.is_some() {
                warn!(
                    "Default value not supported for sensitive prompts in CLI"
                );
            }

            Password::new()
                .with_prompt(message)
                .allow_empty_password(true)
                .interact()
        } else {
            let mut input = Input::new().with_prompt(message).allow_empty(true);
            if let Some(default) = default {
                input = input.default(default);
            }
            input.interact()
        }
        // If we failed to read the value, print an error and report nothing
        .context("Error reading value from prompt")
        .traced()
        .ok()
    }

    async fn prompt_select(
        &self,
        message: String,
        mut options: Vec<SelectOption>,
    ) -> Option<Value> {
        let index = DialoguerSelect::new()
            .with_prompt(message)
            .items(&options)
            .default(0)
            .interact()
            // If we failed to read the value, print an error and report nothing
            .context("Error reading value from select")
            .traced()
            .ok()?;
        Some(options.swap_remove(index).value)
    }
}
