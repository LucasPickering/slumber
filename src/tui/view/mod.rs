mod component;
mod state;
mod theme;
mod util;

use crate::{
    config::{RequestCollection, RequestRecipeId},
    http::RequestRecord,
    tui::{
        input::{Action, InputEngine},
        message::MessageSender,
        view::{
            component::{Component, Draw, Root, UpdateOutcome, ViewMessage},
            state::Notification,
            theme::Theme,
        },
    },
};
use ratatui::prelude::*;
use std::{fmt::Debug, io::Stdout};
use tracing::{error, trace, trace_span};

type Frame<'a> = ratatui::Frame<'a, CrosstermBackend<Stdout>>;

/// Primary entrypoint for the view. This contains the main draw functions, as
/// well as bindings for externally modifying the view state. We use a component
/// architecture based on React, meaning the view is responsible for managing
/// its own state. Certain global state (e.g. the request repository) is managed
/// by the controll and exposed via message passing.
#[derive(Debug)]
pub struct View {
    messages_tx: MessageSender,
    theme: Theme,
    root: Root,
}

impl View {
    pub fn new(
        collection: &RequestCollection,
        messages_tx: MessageSender,
    ) -> Self {
        Self {
            // State
            messages_tx,
            theme: Theme::default(),
            root: Root::new(collection),
        }
    }

    /// Draw the view to screen. This needs access to the input engine in order
    /// to render input bindings as help messages to the user.
    pub fn draw(&self, input_engine: &InputEngine, frame: &mut Frame) {
        self.root.draw(
            &RenderContext {
                input_engine,
                theme: &self.theme,
            },
            (),
            frame,
            frame.size(),
        )
    }

    /// New HTTP request was spawned
    pub fn start_request(&mut self, recipe_id: RequestRecipeId) {
        self.handle_message(ViewMessage::HttpRequest { recipe_id });
    }

    /// An HTTP request succeeded
    pub fn finish_request(&mut self, record: RequestRecord) {
        self.handle_message(ViewMessage::HttpResponse { record });
    }

    /// An HTTP request failed
    pub fn fail_request(
        &mut self,
        recipe_id: RequestRecipeId,
        error: anyhow::Error,
    ) {
        self.handle_message(ViewMessage::HttpError { recipe_id, error });
    }

    /// Historical request was loaded from the repository
    pub fn load_request(&mut self, record: RequestRecord) {
        self.handle_message(ViewMessage::HttpLoad { record });
    }

    /// An error occurred somewhere and the user should be shown a popup
    pub fn set_error(&mut self, error: anyhow::Error) {
        self.handle_message(ViewMessage::Error(error));
    }

    /// Send an informational notification to the user
    pub fn notify(&mut self, message: String) {
        let notification = Notification::new(message);
        self.handle_message(ViewMessage::Notify(notification));
    }

    /// Update the view according to an input action from the user
    pub fn handle_input(&mut self, action: Action) {
        self.handle_message(ViewMessage::Input(action))
    }

    /// Process a view message by passing it to the root component and letting
    /// it pass it down the tree
    fn handle_message(&mut self, message: ViewMessage) {
        let span = trace_span!("View message", ?message);
        span.in_scope(|| {
            match self.root.update_all(message) {
                UpdateOutcome::Consumed => {}
                // Consumer didn't eat the message - huh?
                UpdateOutcome::Propagate(_) => {
                    error!("View message was unhandled");
                }
                // Consumer wants to trigger a new event
                UpdateOutcome::SideEffect(m) => {
                    trace!(message = ?m, "View message produced side-effect");
                    self.messages_tx.send(m);
                }
            }
        });
    }
}

/// Global readonly data that various components need during rendering
#[derive(Debug)]
struct RenderContext<'a> {
    pub input_engine: &'a InputEngine,
    pub theme: &'a Theme,
}
