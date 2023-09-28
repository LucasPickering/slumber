//! Logic related to input handling. This is considered part of the controller.

use crate::tui::{
    state::{AppState, Message},
    view::{
        EnvironmentListPane, ErrorPopup, RecipeListPane, RequestPane,
        ResponsePane,
    },
};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use derive_more::Display;
use std::{
    collections::HashMap,
    fmt::{Debug, Display},
    sync::OnceLock,
};
use tracing::trace;

static INSTANCE: OnceLock<InputManager> = OnceLock::new();

/// Top-level input manager. This is the entrypoint into the input management
/// system. It delegates out to certain children based on UI state.
pub struct InputManager {
    bindings: HashMap<Action, InputBinding>,
}

impl InputManager {
    fn new() -> Self {
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
                InputBinding::new(KeyCode::Char(' '), Action::Interact),
                InputBinding::new(KeyCode::Esc, Action::Close),
            ]
            .into_iter()
            .map(|binding| (binding.action, binding))
            .collect(),
        }
    }

    pub fn instance() -> &'static Self {
        INSTANCE.get_or_init(Self::new)
    }

    /// Get the binding associated with a particular action
    pub fn binding(&self, action: Action) -> Option<InputBinding> {
        self.bindings.get(&action).copied()
    }

    pub fn handle_event(&self, state: &mut AppState, event: Event) {
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
                .find(|binding| binding.matches(&key))
                .map(|binding| binding.action);

            if let Some(action) = action {
                trace!("Input action {action:?}");
                self.apply_action(state, action);
            }
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
    /// Close the current popup
    Close,
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
            KeyCode::Char(' ') => write!(f, "<space>"),
            KeyCode::Char(c) => write!(f, "<{c}>"),
            // Punting on everything else until we need it
            _ => write!(f, "???"),
        }
    }
}

/// A binding between an action and the change it makes to state
pub struct OutcomeBinding {
    pub action: Action,
    pub mutator: Mutator,
}

/// A function to mutate state based on an input action
type Mutator = &'static dyn Fn(&mut AppState);

impl OutcomeBinding {
    fn new(action: Action, mutator: Mutator) -> Self {
        Self { action, mutator }
    }
}

/// A major item in the UI, which can receive input and be drawn to the screen.
/// Each of these types should be a **singleton**. There are assumptions that
/// will break if we start duplicating types.
pub trait InputTarget {
    /// Modify app state based on the given action. Sync actions should modify
    /// state directly. Async actions should queue messages to be handled later.
    fn apply_action(&self, state: &mut AppState, action: Action) {
        let mutator = self
            .actions(state)
            .into_iter()
            .find(|app| app.action == action)
            .map(|app| app.mutator);
        if let Some(mutator) = mutator {
            mutator(state);
        }
    }

    /// Get a list of mappings that will modify the state. This needs to return
    /// a list of available actions so it can be used to show help text.
    fn actions(&self, state: &AppState) -> Vec<OutcomeBinding>;
}

impl InputTarget for InputManager {
    fn actions(&self, state: &AppState) -> Vec<OutcomeBinding> {
        let mut mappings: Vec<OutcomeBinding> = vec![
            OutcomeBinding::new(Action::Quit, &|state| state.quit()),
            OutcomeBinding::new(Action::ReloadCollection, &|state| {
                state.messages_tx.send(Message::StartReloadCollection)
            }),
        ];
        mappings.extend(state.input_handler().actions(state));
        mappings
    }
}

impl InputTarget for EnvironmentListPane {
    fn actions(&self, _: &AppState) -> Vec<OutcomeBinding> {
        vec![
            OutcomeBinding::new(Action::FocusPrevious, &|state| {
                state.ui.selected_pane.previous()
            }),
            OutcomeBinding::new(Action::FocusNext, &|state| {
                state.ui.selected_pane.next()
            }),
            OutcomeBinding::new(Action::Up, &|state| {
                state.ui.environments.previous()
            }),
            OutcomeBinding::new(Action::Down, &|state| {
                state.ui.environments.next()
            }),
        ]
    }
}

impl InputTarget for RecipeListPane {
    fn actions(&self, _: &AppState) -> Vec<OutcomeBinding> {
        vec![
            OutcomeBinding::new(Action::FocusPrevious, &|state| {
                state.ui.selected_pane.previous()
            }),
            OutcomeBinding::new(Action::FocusNext, &|state| {
                state.ui.selected_pane.next()
            }),
            OutcomeBinding::new(Action::Up, &|state| {
                state.ui.recipes.previous()
            }),
            OutcomeBinding::new(Action::Down, &|state| state.ui.recipes.next()),
            OutcomeBinding::new(Action::Interact, &|state| {
                state.messages_tx.send(Message::HttpSendRequest)
            }),
        ]
    }
}

impl InputTarget for RequestPane {
    fn actions(&self, _: &AppState) -> Vec<OutcomeBinding> {
        vec![
            OutcomeBinding::new(Action::FocusPrevious, &|state| {
                state.ui.selected_pane.previous()
            }),
            OutcomeBinding::new(Action::FocusNext, &|state| {
                state.ui.selected_pane.next()
            }),
            OutcomeBinding::new(Action::Left, &|state| {
                state.ui.request_tab.previous()
            }),
            OutcomeBinding::new(Action::Right, &|state| {
                state.ui.request_tab.next()
            }),
        ]
    }
}

impl InputTarget for ResponsePane {
    fn actions(&self, _: &AppState) -> Vec<OutcomeBinding> {
        vec![
            OutcomeBinding::new(Action::FocusPrevious, &|state| {
                state.ui.selected_pane.previous()
            }),
            OutcomeBinding::new(Action::FocusNext, &|state| {
                state.ui.selected_pane.next()
            }),
            OutcomeBinding::new(Action::Left, &|state| {
                state.ui.response_tab.previous()
            }),
            OutcomeBinding::new(Action::Right, &|state| {
                state.ui.response_tab.next()
            }),
        ]
    }
}

impl InputTarget for ErrorPopup {
    fn actions(&self, _: &AppState) -> Vec<OutcomeBinding> {
        let clear_error: Mutator = &|state| state.clear_error();
        vec![
            OutcomeBinding::new(Action::Interact, clear_error),
            OutcomeBinding::new(Action::Close, clear_error),
        ]
    }
}
