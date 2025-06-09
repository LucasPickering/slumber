//! Async message passing! This is how inputs and other external events trigger
//! state updates.

use crate::{util::TempFile, view::Confirm};
use anyhow::Context;
use derive_more::From;
use mime::Mime;
use slumber_config::Action;
use slumber_core::{
    collection::{Collection, ProfileId, RecipeId},
    http::{
        BuildOptions, Exchange, RequestBuildError, RequestError, RequestId,
        RequestRecord,
    },
    render::{Prompt, ResponseChannel, Select},
};
use slumber_template::{RenderedChunk, Template};
use slumber_util::ResultTraced;
use std::{fmt::Debug, sync::Arc};
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

/// A message triggers some *asynchronous* action. Most state modifications can
/// be made synchronously by the input handler, but some require async handling
/// at the top level. Messages can be triggered from anywhere (via the TUI
/// context), but are all handled by the top-level controller.
#[derive(derive_more::Debug)]
pub enum Message {
    /// Clear the terminal. Use this before deferring to a subprocess
    ClearTerminal,

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
    CopyRequestUrl,
    /// Render request body from the selected recipe, then copy rendered text
    CopyRequestBody,
    /// Render request, then generate an equivalent cURL command and copy it
    CopyRequestCurl,
    /// Copy some text to the clipboard
    CopyText(String),

    /// Trigger a redraw. This should be called whenever we have reason to
    /// believe the UI may have changed due to a background task
    Draw,

    /// An error occurred in some async process and should be shown to the user
    Error { error: anyhow::Error },

    /// Open a file in the user's external editor
    FileEdit {
        file: TempFile,
        /// Function to call once the edit is done. The original file will be
        /// passed back so the caller can read its contents before it gets
        /// dropped+deleted.
        #[debug(skip)]
        on_complete: Callback<TempFile>,
    },
    /// Open a file to be viewed in the user's external pager
    FileView {
        file: TempFile,
        /// MIME type of the file being viewed
        mime: Option<Mime>,
    },

    /// Launch an HTTP request from the given recipe/profile.
    HttpBeginRequest,
    /// Announce that we've started building an HTTP request that was triggered
    /// by another request.
    HttpBuildingTriggered {
        id: RequestId,
        profile_id: Option<ProfileId>,
        recipe_id: RecipeId,
    },
    /// Request failed to build
    HttpBuildError { error: Arc<RequestBuildError> },
    /// We launched the HTTP request
    HttpLoading { request: Arc<RequestRecord> },
    /// The HTTP request either succeeded or failed. We don't need to store the
    /// recipe ID here because it's in the inner container already. Combining
    /// these two cases saves a bit of boilerplate. The error must be wrapped
    /// in `Arc` because it may need to be shared. Triggered requests need their
    /// error returned to the template engine, but also need to be inserted into
    /// the request store.
    HttpComplete(Result<Exchange, Arc<RequestError>>),
    /// Cancel an HTTP request
    HttpCancel(RequestId),
    /// Get the most recent _completed_ request for a recipe+profile combo
    HttpGetLatest {
        profile_id: Option<ProfileId>,
        recipe_id: RecipeId,
        channel: ResponseChannel<Option<Exchange>>,
    },

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

    /// Save a response body to a file. This will trigger a process to prompt
    /// the user for a file name
    SaveResponseBody {
        request_id: RequestId,
        /// If the response body has been modified in-TUI (via prettification
        /// or querying), pass whatever the user sees here. Otherwise pass
        /// `None`, and the original response bytes will be used.
        data: Option<String>,
    },

    /// Show a select list to the user, asking them to choose an item
    /// Use the included channel to return the selection.
    SelectStart(Select),

    /// Render a template string, to be previewed in the UI. Ideally this could
    /// be launched directly by the component that needs it, but only the
    /// controller has the data needed to build the template context. The given
    /// callback will be called with the outcome (including inline errors).
    ///
    /// By holding a callback here, we avoid having to plumb the result all the
    /// way back down the component tree.
    TemplatePreview {
        template: Template,
        #[debug(skip)]
        on_complete: Callback<Vec<RenderedChunk>>,
    },
}

/// A static callback included in a message
pub type Callback<T> = Box<dyn 'static + Send + Sync + FnOnce(T)>;

/// Configuration that defines how to render a request
#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq))]
pub struct RequestConfig {
    pub profile_id: Option<ProfileId>,
    pub recipe_id: RecipeId,
    pub options: BuildOptions,
}
