//! Logic related to input handling. This is considered part of the controller.

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use derive_more::Display;
use std::{
    collections::HashMap,
    fmt::{Debug, Display},
};
use tracing::trace;

/// Top-level input manager. This handles things like bindings and mapping
/// events to actions, but then the actions are actually processed by the view.
#[derive(Debug)]
pub struct InputEngine {
    bindings: HashMap<Action, InputBinding>,
}

impl InputEngine {
    pub fn new() -> Self {
        Self {
            bindings: [
                InputBinding {
                    action: Action::Quit,
                    primary: KeyCombination {
                        key_code: KeyCode::Char('q'),
                        modifiers: KeyModifiers::NONE,
                    },
                    secondary: Some(KeyCombination {
                        key_code: KeyCode::Char('c'),
                        modifiers: KeyModifiers::CONTROL,
                    }),
                },
                InputBinding::new(KeyCode::Char('r'), Action::ReloadCollection),
                InputBinding::new(KeyCode::BackTab, Action::FocusPrevious),
                InputBinding::new(KeyCode::Tab, Action::FocusNext),
                InputBinding::new(KeyCode::Up, Action::Up),
                InputBinding::new(KeyCode::Down, Action::Down),
                InputBinding::new(KeyCode::Left, Action::Left),
                InputBinding::new(KeyCode::Right, Action::Right),
                InputBinding::new(KeyCode::Enter, Action::Interact),
                InputBinding::new(KeyCode::Esc, Action::Cancel),
            ]
            .into_iter()
            .map(|binding| (binding.action, binding))
            .collect(),
        }
    }

    /// Get the binding associated with a particular action. Useful for mapping
    /// input in reverse, when showing available bindings to the user.
    pub fn binding(&self, action: Action) -> Option<InputBinding> {
        self.bindings.get(&action).copied()
    }

    /// Convert an input event into its bound action, if any
    pub fn action(&self, event: &Event) -> Option<Action> {
        if let Event::Key(
            key @ KeyEvent {
                kind: KeyEventKind::Press,
                ..
            },
        ) = event
        {
            // Scan all bindings for a match
            let action = self
                .bindings
                .values()
                .find(|binding| binding.matches(key))
                .map(|binding| binding.action);

            if let Some(action) = action {
                trace!("Input action {action:?}");
            }

            action
        } else {
            None
        }
    }
}

/// An input action from the user. This is context-agnostic; the action may not
/// actually mean something in the current app context. This type is just an
/// abstraction to map all possible input events to the things we actually
/// care about handling.
///
/// This is a middle abstraction layer between the input ([KeyCombination]) and
/// the output ([Mutator]).
#[derive(Copy, Clone, Debug, Display, Eq, Hash, PartialEq)]
pub enum Action {
    /// Exit the app
    Quit,
    /// Reload the request collection from the same file as the initial load
    #[display(fmt = "Reload Collection")]
    ReloadCollection,
    /// Focus the next pane
    #[display(fmt = "Next Pane")]
    FocusNext,
    /// Focus the previous pane
    #[display(fmt = "Prev Pane")]
    FocusPrevious,
    Up,
    Down,
    Left,
    Right,
    /// Do a thing. E.g. select an item in a list
    Interact,
    /// Close the current modal/dialog/etc.
    Cancel,
}

/// A mapping from a key input sequence to an action. This can optionally have
/// a secondary binding.
#[derive(Copy, Clone, Debug)]
pub struct InputBinding {
    action: Action,
    primary: KeyCombination,
    secondary: Option<KeyCombination>,
}

impl InputBinding {
    /// Create a binding with only a primary
    const fn new(key_code: KeyCode, action: Action) -> Self {
        Self {
            action,
            primary: KeyCombination {
                key_code,
                modifiers: KeyModifiers::NONE,
            },
            secondary: None,
        }
    }

    fn matches(&self, event: &KeyEvent) -> bool {
        self.primary.matches(event)
            || self
                .secondary
                .map(|secondary| secondary.matches(event))
                .unwrap_or_default()
    }
}

impl Display for InputBinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Don't display secondary binding in help text
        write!(f, "{} {}", self.primary, self.action)
    }
}

/// Key input sequence, which can trigger an action
#[derive(Copy, Clone, Debug)]
struct KeyCombination {
    key_code: KeyCode,
    modifiers: KeyModifiers,
}

impl KeyCombination {
    fn matches(self, event: &KeyEvent) -> bool {
        event.code == self.key_code && event.modifiers.contains(self.modifiers)
    }
}

impl Display for KeyCombination {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.key_code {
            KeyCode::BackTab => write!(f, "<shift+tab>"),
            KeyCode::Tab => write!(f, "<tab>"),
            KeyCode::Up => write!(f, "↑"),
            KeyCode::Down => write!(f, "↓"),
            KeyCode::Left => write!(f, "←"),
            KeyCode::Right => write!(f, "→"),
            KeyCode::Esc => write!(f, "<esc>"),
            KeyCode::Enter => write!(f, "<enter>"),
            KeyCode::Char(' ') => write!(f, "<space>"),
            KeyCode::Char(c) => write!(f, "<{c}>"),
            // Punting on everything else until we need it
            _ => write!(f, "???"),
        }
    }
}
