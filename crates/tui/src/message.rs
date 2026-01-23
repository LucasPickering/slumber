//! Async message passing! This is how inputs and other external events trigger
//! state updates.

use crate::{
    http::{PromptId, PromptReply},
    input::InputEvent,
    util::{ResultReported, TempFile},
    view::Question,
};
use derive_more::From;
use futures::{FutureExt, future::LocalBoxFuture};
use mime::Mime;
use slumber_core::{
    collection::{Collection, ProfileId, RecipeId},
    database::ProfileFilter,
    http::{
        Exchange, RequestBuildError, RequestError, RequestId, RequestRecord,
    },
    render::{Prompt, ReplyChannel},
};
use slumber_template::{RenderedOutput, Template};
use slumber_util::{ResultTraced, yaml::SourceLocation};
use std::{fmt::Debug, path::PathBuf, sync::Arc};
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
        let _ = self.0.send(message).traced();
    }

    /// Spawn a future in a new task on the main thread. See [Message::Spawn]
    pub fn spawn(&self, future: impl 'static + Future<Output = ()>) {
        self.send(Message::Spawn(future.boxed_local()));
    }

    /// Spawn a fallible future in a new task on the main thread
    ///
    /// If the task fails, show the error to the user.
    pub fn spawn_result(
        &self,
        future: impl 'static + Future<Output = anyhow::Result<()>>,
    ) {
        let tx = self.clone();
        let future = async move {
            future.await.reported(&tx);
        };
        self.send(Message::Spawn(future.boxed_local()));
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
    CollectionEdit {
        /// Optional file+line+column to open. If omitted, open the root
        /// collection file to line 1 column 1. The path will *typically* be
        /// the root file but not necessarily, as you can also edit locations
        /// from other referenced files.
        location: Option<SourceLocation>,
    },
    /// Switch to a different collection file. This will start an entirely new
    /// TUI session for the new collection
    CollectionSelect(PathBuf),

    /// Render request URL from a recipe, then copy rendered URL
    CopyRecipe(RecipeCopyTarget),
    /// Copy some text to the clipboard
    CopyText(String),

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

    /// A message that modifies the state of an HTTP request
    Http(HttpMessage),
    /// Get the most recent _completed_ request for a recipe+profile combo
    HttpGetLatest {
        profile_id: Option<ProfileId>,
        recipe_id: RecipeId,
        channel: ReplyChannel<Option<Exchange>>,
    },

    /// User input from the terminal
    Input(InputEvent),

    /// Send an informational notification to the user
    Notify(String),

    /// Ask the user for input to some [Question]. Use the included channel to
    /// return the value.
    ///
    /// This is *not* used for request building; that uses
    /// [HttpMessage::Prompt].
    Question(Question),

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

    /// Spawn a task on the main thread
    ///
    /// Because the task is run on the main thread, it can be `!Send`. This
    /// allows view tasks to access the event queue. The task will be
    /// automatically cancelled when the TUI exits.
    Spawn(#[debug(skip)] LocalBoxFuture<'static, ()>),

    /// Render a template string, to be previewed in the UI. Ideally this could
    /// be launched directly by the component that needs it, but only the
    /// controller has the data needed to build the template context. The given
    /// callback will be called with the outcome (including inline errors).
    ///
    /// By holding a callback here, we avoid having to plumb the result all the
    /// way back down the component tree.
    TemplatePreview {
        template: Template,
        /// Does the consumer support streaming? If so, the output chunks may
        /// contain streams
        can_stream: bool,
        #[debug(skip)]
        on_complete: Callback<RenderedOutput>,
    },
}

impl From<HttpMessage> for Message {
    fn from(value: HttpMessage) -> Self {
        Message::Http(value)
    }
}

/// A message that modifies the state of an HTTP request. These are grouped
/// together to enable the state manager to propagate these changes to the view
/// easily.
#[derive(Debug)]
pub enum HttpMessage {
    /// Build and send an HTTP request based on the current recipe/profile state
    Begin,
    /// An HTTP request was triggered by another request, and is now being built
    Triggered {
        request_id: RequestId,
        profile_id: Option<ProfileId>,
        recipe_id: RecipeId,
    },
    /// A prompt is being rendered in a template, and we need a reply from the
    /// user
    Prompt {
        request_id: RequestId,
        prompt: Prompt,
    },
    /// User has submitted their prompt form in the UI. Replies should be sent
    /// back to the render engine.
    FormSubmit {
        request_id: RequestId,
        replies: Vec<(PromptId, PromptReply)>,
    },
    /// Request failed to build
    ///
    /// The error is wrapped in `Arc` because it may be shared with other tasks.
    BuildError(Arc<RequestBuildError>),
    /// Request was sent and we're now waiting on a response
    Loading(Arc<RequestRecord>),
    /// The HTTP request either succeeded or failed. We don't need to store the
    /// recipe ID here because it's in the inner container already. Combining
    /// these two cases saves a bit of boilerplate. The error must be wrapped
    /// in `Arc` because it may be shared with other tasks.
    Complete(Result<Exchange, Arc<RequestError>>),
    /// Request was cancelled
    Cancel(RequestId),
    /// Delete a request from the store/DB. This executes the delete, so it
    /// should be send *after* the confirmation process.
    DeleteRequest(RequestId),
    /// Delete all requests for a recipe from the store/DB. This executes the
    /// delete, so it should be send *after* the confirmation process.
    DeleteRecipe {
        recipe_id: RecipeId,
        /// Delete requests for just the current profile or all profiles?
        profile_filter: ProfileFilter<'static>,
    },
}

/// Component/form of a recipe to copy to the clipboard
#[derive(Debug, PartialEq)]
pub enum RecipeCopyTarget {
    /// Render request URL from the selected recipe, then copy rendered URL
    Url,
    /// Render request body from the selected recipe, then copy rendered text
    Body,
    /// Copy selected recipe as an equivelent Slumber CLI command
    Cli,
    /// Render request from the selected recipe, then generate an equivalent
    /// cURL command and copy it
    Curl,
    /// Copy selected recipe as Python code that uses the `slumber` package
    Python,
}

/// A static callback included in a message
pub type Callback<T> = Box<dyn 'static + FnOnce(T)>;
