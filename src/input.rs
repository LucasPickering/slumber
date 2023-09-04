//! Logic related to input handling. This is considered part of the controller.

use crate::{
    state::{AppState, Message},
    ui::Element,
};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::{any::Any, fmt::Debug};
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
    /// Go up (e.g in a list)
    Up,
    /// Go down (e.g. in a list)
    Down,
    /// Do a thing. E.g. select an item in a list
    Select,
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
                KeyCode::Char(' ') => Some(Action::Select),
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
pub trait ActionHandler: Any + Debug {
    fn handle_action(&self, state: &mut AppState, action: Action);
}

/// Handle an action globally. Some actions are context-independent, meaning
/// they have the same effect regardless of focus or other context. Others are
/// contextual, and will be forwarded to the focused element.
pub fn handle_action(state: &mut AppState, action: Action) {
    match action {
        // Global events
        Action::Quit => state.quit(),
        Action::FocusPrevious => state.focus_previous(),
        Action::FocusNext => state.focus_next(),

        // Forward context events to the focused element
        other => state.focused_element.clone().handle_action(state, other),
    }
}

impl ActionHandler for Element {
    fn handle_action(&self, state: &mut AppState, action: Action) {
        // TODO use dynamic dispatch to split this up?
        match (self, action) {
            // Ignore global actions
            (_, Action::Quit | Action::FocusNext | Action::FocusPrevious) => {}

            (Element::EnvironmentList, Action::Up) => {
                state.environments.previous()
            }
            (Element::EnvironmentList, Action::Down) => {
                state.environments.next()
            }
            (Element::EnvironmentList, Action::Select) => {}

            (Element::RecipeList, Action::Up) => state.recipes.previous(),
            (Element::RecipeList, Action::Down) => state.recipes.next(),
            (Element::RecipeList, Action::Select) => {
                state.enqueue(Message::SendRequest)
            }

            // Nothing to do on these yet
            (Element::RequestDetail, _) => {}
            (Element::ResponseDetail, _) => {}
        }
    }
}
