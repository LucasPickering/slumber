mod component;
mod state;
mod theme;
mod util;

pub use component::ModalPriority;
pub use state::RequestState;

use crate::{
    config::{RequestCollection, RequestRecipeId},
    tui::{
        input::{Action, InputEngine},
        message::MessageSender,
        view::{
            component::{
                Component, Draw, DrawContext, Event, IntoModal, Root,
                UpdateContext, UpdateOutcome,
            },
            state::Notification,
            theme::Theme,
        },
    },
};
use ratatui::Frame;
use std::fmt::Debug;
use tracing::{error, trace, trace_span};

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
        let mut view = Self {
            messages_tx,
            theme: Theme::default(),
            root: Root::new(collection),
        };
        // Tell the components to wake up
        view.handle_event(Event::Init);
        view
    }

    /// Draw the view to screen. This needs access to the input engine in order
    /// to render input bindings as help messages to the user.
    pub fn draw<'a>(
        &'a self,
        input_engine: &'a InputEngine,
        frame: &'a mut Frame,
    ) {
        let chunk = frame.size();
        self.root.draw(
            &mut DrawContext {
                input_engine,
                theme: &self.theme,
                frame,
            },
            (),
            chunk,
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
        self.handle_event(Event::HttpSetState { recipe_id, state });
    }

    /// Open a new modal. The input can be anything that converts to modal
    /// content
    pub fn open_modal(
        &mut self,
        modal: impl IntoModal + 'static,
        priority: ModalPriority,
    ) {
        self.handle_event(Event::OpenModal {
            modal: Box::new(modal.into_modal()),
            priority,
        });
    }

    /// Send an informational notification to the user
    pub fn notify(&mut self, message: String) {
        let notification = Notification::new(message);
        self.handle_event(Event::Notify(notification));
    }

    /// Update the view according to an input event from the user. If possible,
    /// a bound action is provided which tells us what abstract action the
    /// input maps to.
    pub fn handle_input(
        &mut self,
        event: crossterm::event::Event,
        action: Option<Action>,
    ) {
        self.handle_event(Event::Input { event, action })
    }

    /// Process a view event by passing it to the root component and letting
    /// it pass it down the tree
    fn handle_event(&mut self, event: Event) {
        let span = trace_span!("View event", ?event);
        span.in_scope(|| {
            let mut context = self.update_context();
            match Self::update_all(&mut self.root, &mut context, event) {
                UpdateOutcome::Consumed => {
                    trace!("View event consumed")
                }
                // Consumer didn't eat the event - huh?
                UpdateOutcome::Propagate(_) => {
                    error!("View event was unhandled");
                }
            }
        });
    }

    /// Context object passed to each update call
    fn update_context(&self) -> UpdateContext {
        UpdateContext::new(self.messages_tx.clone())
    }

    /// Update the state of a component *and* its children, starting at the
    /// lowest descendant. Recursively walk up the tree until a component
    /// consumes the event.
    fn update_all(
        component: &mut dyn Component,
        context: &mut UpdateContext,
        mut event: Event,
    ) -> UpdateOutcome {
        // If we have a child, send them the event. If not, eat it ourselves
        for child in component.children() {
            let outcome = Self::update_all(child, context, event); // RECURSION
            if let UpdateOutcome::Propagate(returned) = outcome {
                // Keep going to the next child. It's possible the child
                // returned something other than the original event, which
                // we'll just pass along anyway.
                event = returned;
            } else {
                trace!(%child, "View event consumed");
                return outcome;
            }
        }

        // None of our children handled it, we'll take it ourselves.
        // Message is already traced in the parent span, so don't dupe it.
        let span = trace_span!("Component handling", %component);
        span.in_scope(|| {
            let outcome = component.update(context, event);
            trace!(?outcome);
            outcome
        })
    }
}
