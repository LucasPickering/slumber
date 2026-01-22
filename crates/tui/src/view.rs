mod common;
mod component;
mod context;
mod event;
pub mod persistent;
mod state;
mod styles;
#[cfg(test)]
mod test_util;
mod util;

pub use component::ComponentMap;
pub use context::UpdateContext;
pub use event::Event;
pub use util::{InvalidCollection, PreviewPrompter, Question, TuiPrompter};

use crate::{
    http::{RequestConfig, RequestState, RequestStore},
    message::MessageSender,
    view::{
        component::{Canvas, Component, ComponentExt, Root},
        context::ViewContext,
        persistent::PersistentStore,
    },
};
use indexmap::IndexMap;
use ratatui::{buffer::Buffer, text::Span};
use slumber_config::Config;
use slumber_core::{
    collection::{Collection, ProfileId, RecipeId, ValueTemplate},
    database::CollectionDatabase,
    http::RequestId,
    render::Prompt,
};
use std::{
    fmt::{Debug, Display},
    sync::Arc,
};
use tracing::{trace, trace_span, warn};

/// Primary entrypoint for the view. This contains the main draw functions, as
/// well as bindings for externally modifying the view state. We use a component
/// architecture based on React, meaning the view is responsible for managing
/// its own state. Certain global state (e.g. the database) is managed by the
/// controller and exposed via event passing.
///
/// View state is updated via [event messages](crate::message::Message::Event).
/// Call [handle_event](Self::handle_event) when a view event is received on
/// the message queue.
#[derive(Debug)]
pub struct View {
    /// Root of the component tree
    root: Root,
}

impl View {
    /// Initialize the view. This will build out the entire component tree
    ///
    /// This accepts a loaded collection *or* an error. If the collection fails
    /// to load, we'll show the error and wait for the user to fix it or exit.
    pub fn new(
        config: Arc<Config>,
        collection: Result<Arc<Collection>, InvalidCollection>,
        database: CollectionDatabase,
        messages_tx: MessageSender,
    ) -> Self {
        // If the collection is invalid, just put an empty one in the view
        // context. We *shouldn't* hit any code that tries to use it because
        // we won't be drawing the normal view, but it's easiest just to have
        // it there anyway.
        ViewContext::init(
            config,
            collection.as_ref().map(Arc::clone).unwrap_or_default(),
            database,
            messages_tx,
        );

        Self {
            root: Root::new(collection),
        }
    }

    /// Draw the view to a screen buffer
    ///
    /// Return the map of all drawn components.
    pub fn draw<'f>(&'f self, buffer: &'f mut Buffer) -> Canvas<'f> {
        // If the screen is too small to render anything, don't try. This avoids
        // panics within ratatui from trying to render borders and margins
        // outside the buffer area
        if buffer.area().width > 1 || buffer.area().height > 1 {
            Canvas::draw_all(buffer, &self.root, ())
        } else {
            Canvas::new(buffer) // Empty canvas, no draw
        }
    }

    /// Persist all UI state to the database. This should be called at the end
    /// of each update phase. It does *not* need to be called after each
    /// individual event when multiple events are batched together.
    ///
    /// This takes `&mut self` because we dynamically load children, and those
    /// are always mutable.
    pub fn persist(&mut self, database: &CollectionDatabase) {
        let mut store = PersistentStore::new(database);
        self.root.persist_all(&mut store);
        store.commit();
    }

    /// ID of the selected profile. `None` iff the list is empty
    pub fn selected_profile_id(&self) -> Option<&ProfileId> {
        self.root.selected_profile_id()
    }

    /// Get a definition of the request that should be sent from the current
    /// recipe settings
    pub fn request_config(&self) -> Option<RequestConfig> {
        self.root.request_config()
    }

    /// Get a map of overridden profile fields
    pub fn profile_overrides(&self) -> IndexMap<String, ValueTemplate> {
        self.root.profile_overrides()
    }

    /// Update the displayed request based on a change in HTTP request state.
    /// If the update is not relevant to what's on screen (e.g. an unselected
    /// request was modified), this will do nothing.
    pub fn refresh_request(
        &mut self,
        store: &mut RequestStore,
        disposition: RequestDisposition,
    ) {
        self.root.refresh_request(store, disposition);
    }

    /// Ask the user a [Question]
    pub fn question(&mut self, question: Question) {
        self.root.question(question);
    }

    /// Display an error to the user in a modal
    pub fn error(&mut self, error: anyhow::Error) {
        self.root.error(error);
    }

    /// Display an informational notification to the user
    pub fn notify(&mut self, message: impl ToString) {
        self.root.notify(message.to_string());
    }

    /// Update the view in response to a view event
    pub fn handle_event(&mut self, mut context: UpdateContext, event: Event) {
        trace_span!("Handling event", ?event).in_scope(|| {
            match self.root.update_all(&mut context, event) {
                None => trace!("Event consumed"),
                // Consumer didn't eat the event - huh?
                Some(event) => warn!(?event, "Event was unhandled"),
            }
        });
    }
}

/// A helper for building a UI. It can be converted into some UI element to be
/// drawn.
pub trait Generate {
    type Output<'this>
    where
        Self: 'this;

    /// Build a UI element
    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this;
}

/// Marker trait to pull in a blanket impl of [Generate], which simply calls
/// [ToString::to_string] on the value to create a [ratatui::text::Span].
pub trait ToStringGenerate: Display {}

impl ToStringGenerate for &str {}

impl<T> Generate for &T
where
    T: ToStringGenerate,
{
    type Output<'this>
        = Span<'this>
    where
        Self: 'this;

    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this,
    {
        self.to_string().into()
    }
}

/// Hint to the view about how an HTTP request state update should be handled
#[derive(Debug)]
pub enum RequestDisposition {
    /// A request was changed in some way. If the request is visible, the UI
    /// will need to be updated.
    Change(RequestId),
    /// Multiple requests were changed. If any of these requests is visible, the
    /// UI will need to be updated.
    ChangeAll(Vec<RequestId>),
    /// Updated request should be selected as the active request. The selection
    /// will only be made if the request matches the current recipe/profile. Use
    /// this when a new request is created or a new recipe/profile was selected.
    Select(RequestId),
    /// Display a prompt to the user (e.g. from `prompt()` or `select()`). This
    /// will either open a new prompt form or append to an existing one.
    OpenPrompt {
        /// Recipe being built. We **cannot** just grab this from the request
        /// store based on the request ID, because the request may not be in
        /// the store (e.g. when rendering for a copy action).
        recipe_id: RecipeId,
        request_id: RequestId,
        prompt: Prompt,
    },
}
