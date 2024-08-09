//! Async message passing! This is how inputs and other external events trigger
//! state updates.

use crate::view::Confirm;
use anyhow::Context;
use derive_more::From;
use slumber_config::Action;
use slumber_core::{
    collection::{Collection, ProfileId, RecipeId},
    http::{
        BuildOptions, Exchange, RequestBuildError, RequestError, RequestRecord,
    },
    template::{Prompt, Prompter, Template, TemplateChunk},
    util::ResultTraced,
};
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedSender;
use tracing::trace;

/// Wrapper around a sender for async messages. Cheap to clone and pass around
#[derive(Clone, Debug, From)]
pub struct MessageSender(UnboundedSender<Message>);

impl MessageSender {
    pub fn new(sender: UnboundedSender<Message>) -> Self {
        Self(sender)
    }

    /// Send an async message, to be handled by the main loop
    pub fn send(&self, message: impl Into<Message>) {
        let message: Message = message.into();
        trace!(?message, "Queueing message");
        let _ = self
            .0
            .send(message)
            .context("Error enqueueing message")
            .traced();
    }
}

/// Use the message stream to prompt the user for input when needed for a
/// template. The message will be routed to the view so it can show the prompt,
/// and the given returner will be used to send the submitted value back.
impl Prompter for MessageSender {
    fn prompt(&self, prompt: Prompt) {
        self.send(Message::PromptStart(prompt));
    }
}

/// A message triggers some *asynchronous* action. Most state modifications can
/// be made synchronously by the input handler, but some require async handling
/// at the top level. Messages can be triggered from anywhere (via the TUI
/// context), but are all handled by the top-level controller.
#[derive(derive_more::Debug)]
pub enum Message {
    /// Trigger collection reload
    CollectionStartReload,
    /// Store a reloaded collection value in state
    CollectionEndReload(Collection),
    /// Open the collection in the user's editor
    CollectionEdit,

    /// Show a yes/no confirmation to the user. Use the included channel to
    /// return the value.
    ConfirmStart(Confirm),

    /// Render request URL from a recipe, then copy rendered URL
    CopyRequestUrl(RequestConfig),
    /// Render request body from a recipe, then copy rendered text
    CopyRequestBody(RequestConfig),
    /// Render request, then generate an equivalent cURL command and copy it
    CopyRequestCurl(RequestConfig),
    /// Copy some text to the clipboard
    CopyText(String),

    /// An error occurred in some async process and should be shown to the user
    Error { error: anyhow::Error },

    /// Launch an HTTP request from the given recipe/profile.
    HttpBeginRequest(RequestConfig),
    /// Request failed to build
    HttpBuildError { error: RequestBuildError },
    /// We launched the HTTP request
    HttpLoading { request: Arc<RequestRecord> },
    /// The HTTP request either succeeded or failed. We don't need to store the
    /// recipe ID here because it's in the inner container already. Combining
    /// these two cases saves a bit of boilerplate.
    HttpComplete(Result<Exchange, RequestError>),

    /// User input from the terminal
    Input {
        /// Raw input event
        event: crossterm::event::Event,
        /// Action mapped via input bindings. This is what most consumers use
        action: Option<Action>,
    },

    /// Send an informational notification to the user
    Notify(String),
    /// Show a prompt to the user, asking for some input. Use the included
    /// channel to return the value.
    PromptStart(Prompt),

    /// Exit the program
    Quit,

    /// Save data to a file. Could be binary (e.g. image) or encoded text
    SaveFile {
        /// A suggestion for the file name. User will have the opportunity to
        /// change this
        default_path: Option<String>,
        /// Data to save
        data: Vec<u8>,
    },

    /// Render a template string, to be previewed in the UI. Ideally this could
    /// be launched directly by the component that needs it, but only the
    /// controller has the data needed to build the template context. The given
    /// callback will be called with the outcome (including inline errors).
    ///
    /// By holding a callback here, we avoid having to plumb the result all the
    /// way back down the component tree.
    TemplatePreview {
        template: Template,
        profile_id: Option<ProfileId>,
        #[debug(skip)]
        on_complete:
            Box<dyn 'static + Send + Sync + FnOnce(Vec<TemplateChunk>)>,
    },
}

/// Configuration that defines how to render a request
#[derive(Clone, Debug)]
#[cfg_attr(test, derive(PartialEq))]
pub struct RequestConfig {
    pub profile_id: Option<ProfileId>,
    pub recipe_id: RecipeId,
    pub options: BuildOptions,
}
