//! Async message passing! This is how inputs and other external events trigger
//! state updates.

use crate::{
    collection::{Collection, ProfileId, RecipeId},
    http::{
        RecipeOptions, Request, RequestBuildError, RequestError, RequestRecord,
    },
    template::{Prompt, Prompter, Template, TemplateChunk},
    util::ResultExt,
};
use anyhow::Context;
use derive_more::From;
use std::sync::{Arc, OnceLock};
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
/// at the top level. The controller is responsible for both triggering and
/// handling messages.
#[derive(Debug)]
pub enum Message {
    /// Trigger collection reload
    CollectionStartReload,
    /// Store a reloaded collection value in state
    CollectionEndReload(Collection),
    /// Open the collection in the user's editor
    CollectionEdit,

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
    HttpBuildError {
        profile_id: Option<ProfileId>,
        recipe_id: RecipeId,
        error: RequestBuildError,
    },
    /// We launched the HTTP request
    HttpLoading {
        profile_id: Option<ProfileId>,
        recipe_id: RecipeId,
        request: Arc<Request>,
    },
    /// The HTTP request either succeeded or failed. We don't need to store the
    /// recipe ID here because it's in the inner container already. Combining
    /// these two cases saves a bit of boilerplate.
    HttpComplete(Result<RequestRecord, RequestError>),

    /// Show a prompt to the user, asking for some input. Use the included
    /// channel to return the value.
    PromptStart(Prompt),

    /// Exit the program
    Quit,

    /// Load the most recent response for a recipe from the database
    RequestLoad {
        profile_id: Option<ProfileId>,
        recipe_id: RecipeId,
    },

    /// Render a template string, to be previewed in the UI. Ideally this could
    /// be launched directly by the component that needs it, but only the
    /// controller has the data needed to build the template context. The
    /// result (including inline errors) will be written back to the given
    /// cell.
    ///
    /// By specifying the destination inline, we avoid having to plumb the
    /// result all the way back down the component tree.
    TemplatePreview {
        template: Template,
        profile_id: Option<ProfileId>,
        destination: Arc<OnceLock<Vec<TemplateChunk>>>,
    },
}

/// Configuration that defines how to render a request
#[derive(Clone, Debug)]
pub struct RequestConfig {
    pub profile_id: Option<ProfileId>,
    pub recipe_id: RecipeId,
    pub options: RecipeOptions,
}
