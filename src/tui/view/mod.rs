mod component;
mod state;
mod theme;
mod util;

pub use state::RequestState;

use crate::{
    config::{RequestCollection, RequestRecipeId},
    template::Prompt,
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
use crossterm::event::Event;
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

    /// Update the request state for the given recipe. The state will only be
    /// updated if this is a new request or it matches the current request for
    /// this recipe. We only store one request per recipe at a time.
    pub fn set_request_state(
        &mut self,
        recipe_id: RequestRecipeId,
        state: RequestState,
    ) {
        self.handle_message(ViewMessage::HttpSetState { recipe_id, state });
    }

    /// Prompt the user to enter some input
    pub fn set_prompt(&mut self, prompt: Prompt) {
        self.handle_message(ViewMessage::Prompt(prompt))
    }

    /// An error occurred somewhere and the user should be shown a modal
    pub fn set_error(&mut self, error: anyhow::Error) {
        self.handle_message(ViewMessage::Error(error));
    }

    /// Send an informational notification to the user
    pub fn notify(&mut self, message: String) {
        let notification = Notification::new(message);
        self.handle_message(ViewMessage::Notify(notification));
    }

    /// Update the view according to an input event from the user. If possible,
    /// a bound action is provided which tells us what abstract action the
    /// input maps to.
    pub fn handle_input(&mut self, event: Event, action: Option<Action>) {
        self.handle_message(ViewMessage::InputAction { event, action })
    }

    /// Process a view message by passing it to the root component and letting
    /// it pass it down the tree
    fn handle_message(&mut self, message: ViewMessage) {
        let span = trace_span!("View message", ?message);
        span.in_scope(|| {
            match self.root.update_all(message) {
                UpdateOutcome::Consumed => {
                    trace!("View message consumed")
                }
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
