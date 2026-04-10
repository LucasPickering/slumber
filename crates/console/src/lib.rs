//! Utilities for console-based Slumber frontends

use anyhow::Context as _;
use async_trait::async_trait;
use dialoguer::{Input, Password, Select};
use slumber_core::render::{Prompter, SelectOption};
use slumber_template::Value;
use slumber_util::ResultTracedAnyhow;
use tokio::sync::Mutex;
use tracing::warn;

/// A proxy for a lock on stdout
///
/// Forces prompts to run one at a time, so they don't fight over the console.
/// This is a static instead of being in [ConsolePrompter] because there could
/// theoretically be multiple prompters running simultaneously.
static CONSOLE_LOCK: Mutex<()> = Mutex::const_new(());

/// Prompt the user for input on the console
///
/// This uses [dialoguer] to generate the prompts. Since its API is blocking,
/// this runs each prompt in a tokio blocking thread.
#[derive(Debug)]
pub struct ConsolePrompter;

impl ConsolePrompter {
    /// Spawn a blocking prompt in a background task
    async fn spawn_prompt<T, F>(&self, f: F) -> Option<T>
    where
        T: 'static + Send,
        F: 'static + FnOnce() -> anyhow::Result<T> + Send,
    {
        let guard = CONSOLE_LOCK.lock().await;
        let result = tokio::task::spawn_blocking(f)
            .await
            .context("Prompt panicked")
            .flatten()
            .traced();
        // Make sure the guard is held while the task is running. Since the lock
        // doesn't actually contain the locked resource, I wanna be careful.
        drop(guard);
        result.ok()
    }
}

#[async_trait(?Send)]
impl Prompter for ConsolePrompter {
    async fn prompt_text(
        &self,
        message: String,
        default: Option<String>,
        sensitive: bool,
    ) -> Option<String> {
        self.spawn_prompt(move || {
            if sensitive {
                // Dialoguer doesn't support default values here so there's
                // nothing we can do
                if default.is_some() {
                    warn!(
                        "Default value not supported for sensitive CLI prompts"
                    );
                }

                Password::new()
                    .with_prompt(message)
                    .allow_empty_password(true)
                    .interact()
            } else {
                let mut input =
                    Input::new().with_prompt(message).allow_empty(true);
                if let Some(default) = default {
                    input = input.default(default);
                }
                input.interact()
            }
            // If we failed to read the value, print an error and report nothing
            .context("Error reading value from prompt")
        })
        .await
    }

    async fn prompt_select(
        &self,
        message: String,
        mut options: Vec<SelectOption>,
    ) -> Option<Value> {
        self.spawn_prompt(move || {
            let index = Select::new()
                .with_prompt(message)
                .items(&options)
                .default(0)
                .interact()
                // If we failed to read the value, print an error and report
                // nothing
                .context("Error reading value from select")?;
            Ok(options.swap_remove(index).value)
        })
        .await
    }
}
