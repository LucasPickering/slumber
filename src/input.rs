//! Logic related to input handling. This is considered part of the controller.

use crate::{
    state::{AppState, Message},
    view::{EnvironmentListPane, RecipeListPane, RequestPane, ResponsePane},
};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::fmt::Debug;
use tracing::trace;

/// An input action from the user. This is context-agnostic; the action may not
/// actually mean something in the current app context. This type is just an
/// abstraction to map all possible input events to the things we actually
/// care about handling.
#[derive(Debug)]
pub enum Action {
    /// Exit the app
    Quit,
    /// Focus the next pane
    FocusNext,
    /// Focus the previous pane
    FocusPrevious,
    Up,
    Down,
    Left,
    Right,
    /// Do a thing. E.g. select an item in a list
    SendRequest,
}

impl Action {
    /// Map a generic input event into a specific action. This narrows the event
    /// down to either something we know we care about, or nothing.
    pub fn from_event(event: Event) -> Option<Self> {
        let action = if let Event::Key(
            key @ KeyEvent {
                kind: KeyEventKind::Press,
                ..
            },
        ) = event
        {
            match key.code {
                // q or ctrl-c both quit
                KeyCode::Char('q') => Some(Action::Quit),
                KeyCode::Char('c')
                    if key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    Some(Action::Quit)
                }
                KeyCode::BackTab => Some(Action::FocusPrevious),
                KeyCode::Tab => Some(Action::FocusNext),
                KeyCode::Up => Some(Action::Up),
                KeyCode::Down => Some(Action::Down),
                KeyCode::Left => Some(Action::Left),
                KeyCode::Right => Some(Action::Right),
                KeyCode::Char(' ') => Some(Action::SendRequest),
                _ => None,
            }
        } else {
            None
        };

        if let Some(action) = &action {
            trace!("Input action {action:?}");
        }

        action
    }
}

/// A major item in the UI, which can receive input and be drawn to the screen.
/// Each of these types should be a **singleton**. There are assumptions that
/// will break if we start duplicating types.
pub trait InputHandler {
    /// Modify app state based on the given action. Sync actions should modify
    /// state directly, while async ones should queue messages, to be handled
    /// later.
    fn handle_action(&self, state: &mut AppState, action: Action);
}

/// Handle an action globally. Some actions are context-independent, meaning
/// they have the same effect regardless of focus or other context. Others are
/// contextual, and will be forwarded to the focused element.
pub fn handle_action(state: &mut AppState, action: Action) {
    match action {
        // Global events
        Action::Quit => state.quit(),
        Action::FocusPrevious => state.focused_pane.previous(),
        Action::FocusNext => state.focused_pane.next(),
        Action::SendRequest => state.enqueue(Message::SendRequest),

        // Forward context events to the focused element
        other => state
            .focused_pane
            .selected()
            .input_handler()
            .handle_action(state, other),
    }
}

impl InputHandler for EnvironmentListPane {
    fn handle_action(&self, state: &mut AppState, action: Action) {
        match action {
            Action::Up => state.environments.previous(),
            Action::Down => state.environments.next(),
            _ => {}
        }
    }
}

impl InputHandler for RecipeListPane {
    fn handle_action(&self, state: &mut AppState, action: Action) {
        match action {
            Action::Up => state.recipes.previous(),
            Action::Down => state.recipes.next(),
            _ => {}
        }
    }
}

impl InputHandler for RequestPane {
    fn handle_action(&self, _state: &mut AppState, _action: Action) {}
}

impl InputHandler for ResponsePane {
    fn handle_action(&self, state: &mut AppState, action: Action) {
        match action {
            Action::Left => state.response_tab.previous(),
            Action::Right => state.response_tab.next(),
            _ => {}
        }
    }
}
