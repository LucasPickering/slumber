//! Async message passing! This is how inputs and other external events trigger
//! state updates.

use crate::{
    input::InputEvent,
    util::{ResultReported, TempFile},
    view::{Event, Question},
};
use anyhow::Context;
use futures::{FutureExt, future::LocalBoxFuture};
use mime::Mime;
use slumber_core::{
    collection::{Collection, CollectionError, ProfileId, RecipeId},
    database::ProfileFilter,
    http::{
        Exchange, RequestBuildError, RequestError, RequestId, RequestRecord,
    },
    render::{Prompt, ReplyChannel},
};
use slumber_template::{RenderedOutput, Template};
use slumber_util::yaml::SourceLocation;
use std::{
    cell::RefCell,
    collections::VecDeque,
    fmt::Debug,
    path::PathBuf,
    rc::Rc,
    sync::Arc,
    task::{Poll, Waker},
};
use tokio::task;
use tracing::trace;

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
    ///
    /// If the result is `Err`, we'll switch to an error state
    CollectionEndReload(Result<Collection, CollectionError>),
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

    /// An event originated by the view that can modify view state
    ///
    /// Unlike all other [Message] variants, events are handled by individual
    /// components rather than the root TUI loop.
    Event(Event),

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

    /// Redraw the screen with no updates
    Tick,
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
    /// Build and send an HTTP request as a clone of a previous request
    Resend(RequestId),
    /// An HTTP request was triggered by another request, and is now being built
    Triggered {
        request_id: RequestId,
        profile_id: Option<ProfileId>,
        recipe_id: RecipeId,
    },
    /// A prompt is being rendered in a template, and we need a reply from the
    /// user
    Prompt {
        recipe_id: RecipeId,
        request_id: RequestId,
        prompt: Prompt,
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

/// Component/format of a recipe to copy to the clipboard
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

/// Create a new [Message] queue, returning the `(sender, receiver)` pair
///
/// This is *not* a generic message queue. It is specifically used for sending
/// and receiving [Message]s, which drive the main TUI loop.
///
/// This is an mpsc (multi-producer, single-consumer) queue, so the sender can
/// be cloned freely while the receiver cannot. This is very similar to tokio's
/// `mpsc` channel, with some differences:
/// - This uses `Rc<RefCell<_>>` instead of atomic primitives because our
///   futures are all `!Send`. Probably gives a tiny performance benefit
/// - Because this is just a `VecDeque` underneath, it's fully inspectable for
///   tests.
pub fn queue() -> (MessageSender, MessageReceiver) {
    let queue = MessageQueue::default();
    (MessageSender(queue.clone()), MessageReceiver(queue))
}

/// Cloneable transmitter for the mpsc message queue; see [queue]
#[derive(Clone, Debug)]
pub struct MessageSender(MessageQueue);

impl MessageSender {
    /// Send an async message, to be handled by the main loop
    pub fn send(&self, message: impl Into<Message>) {
        let message: Message = message.into();
        trace!(?message, "Queueing message");
        self.0.push(message);
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

    /// Spawn CPU-bound work on a blocking thread
    ///
    /// The output of the blocking work will be passed back to the main thread.
    /// It's then handed to the `into_message` function, which will pack the
    /// data into a [Message] so it can be sent back to the main TUI loop and
    /// used to update state.
    pub fn spawn_blocking<T: 'static + Send>(
        &self,
        blocking: impl 'static + FnOnce() -> T + Send,
        into_message: impl 'static + FnOnce(T) -> Message,
    ) {
        // We need two tasks here:
        // - Inner blocking task does the CPU work on another thread
        // - Outer local task runs on the main thread and just waits on the
        //   inner task. Once it's done, it sends the outcome back to the loop
        // We can't do the CPU work on the local task because it would block the
        // loop, and we can't send the message from the blocking thread because
        // messages are !Send
        let messages_tx = self.clone();
        self.spawn_result(async move {
            let out = task::spawn_blocking(blocking)
                .await
                .context("Blocking thread panicked")?;
            messages_tx.send(into_message(out));
            Ok(())
        });
    }
}

/// Receiver for the mpsc message queue; see [queue]
#[derive(Debug)]
pub struct MessageReceiver(MessageQueue);

impl MessageReceiver {
    /// Does the queue have no messages?
    pub fn is_empty(&self) -> bool {
        self.0.inner.borrow().queue.is_empty()
    }

    /// Pop a message off the queue
    ///
    /// This will wait indefinitely until the next message is available.
    pub async fn pop(&self) -> Message {
        let queue = self.0.clone();
        std::future::poll_fn(move |cx: &mut std::task::Context<'_>| {
            if let Some(message) = queue.pop() {
                Poll::Ready(message)
            } else {
                // Store the waker so we'll get notified for a new message
                queue.set_waker(cx.waker().clone());
                Poll::Pending
            }
        })
        .await
    }
}

/// Test-only helpers
#[cfg(test)]
impl MessageReceiver {
    /// Assert that the message queue is empty
    pub fn assert_empty(&self) {
        if let Some(message) = self.0.pop() {
            panic!("Expected message queue to be empty, but got {message:?}");
        }
    }

    /// Pop the next message off the queue immediately, or `None` if the queue
    /// is empty
    pub fn try_pop(&self) -> Option<Message> {
        self.0.pop()
    }

    /// Pop the next message off the queue, waiting if empty. This will wait
    /// with a timeout to prevent missing messages from blocking a test forever.
    /// If the timeout expires, return `None`.
    pub async fn pop_timeout(&self) -> Option<Message> {
        use std::time::Duration;
        tokio::time::timeout(Duration::from_millis(1000), self.pop())
            .await
            .ok()
    }

    /// Clear all messages in the queue
    pub fn clear(&self) {
        self.0.inner.borrow_mut().queue.clear();
    }
}

/// A `!Send` message queue implemented with refcounting and interior mutability
#[derive(Clone, Debug)]
struct MessageQueue {
    inner: Rc<RefCell<MessageQueueInner>>,
}

impl MessageQueue {
    fn push(&self, message: Message) {
        let mut inner = self.inner.borrow_mut();
        inner.queue.push_back(message);
        // Notify receiver that a message is available
        if let Some(waker) = inner.waker.take() {
            waker.wake();
        }
    }

    fn pop(&self) -> Option<Message> {
        self.inner.borrow_mut().queue.pop_front()
    }

    fn set_waker(&self, waker: Waker) {
        self.inner.borrow_mut().waker = Some(waker);
    }
}

impl Default for MessageQueue {
    fn default() -> Self {
        let inner = MessageQueueInner {
            queue: VecDeque::with_capacity(10),
            waker: None,
        };
        Self {
            inner: Rc::new(RefCell::new(inner)),
        }
    }
}

#[derive(Debug)]
struct MessageQueueInner {
    queue: VecDeque<Message>,
    /// Store the waker for the most recent call to [MessageReceiver::pop].
    /// There can only ever be one listener on the queue, so there can't be
    /// concurrent `pop()`s. Whenever a message is pushed onto the queue,
    /// notify this waker so the main loop task can wake up and handle the
    /// message.
    waker: Option<Waker>,
}
