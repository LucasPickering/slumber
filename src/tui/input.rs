//! Logic related to input handling. This is considered part of the controller.

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton,
    MouseEvent, MouseEventKind,
};
use derive_more::Display;
use indexmap::IndexMap;
use std::fmt::Debug;
use tracing::trace;

/// Top-level input manager. This handles things like bindings and mapping
/// events to actions, but then the actions are actually processed by the view.
#[derive(Debug)]
pub struct InputEngine {
    /// Intuitively this should be binding:action, but we can't look up a
    /// binding from the map based on an input event, because event<=>binding
    /// matching is more nuanced that simple equality (e.g. bonus modifiers
    /// keys can be ignored). We have to iterate over map when checking inputs,
    /// but keying by action at least allows us to look up action=>binding for
    /// help text.
    bindings: IndexMap<Action, InputBinding>,
}

impl InputEngine {
    pub fn new() -> Self {
        Self {
            bindings: [
                InputBinding::new(KeyCode::Char('q'), Action::Quit),
                InputBinding::new(
                    KeyCombination {
                        key_code: KeyCode::Char('c'),
                        modifiers: KeyModifiers::CONTROL,
                    },
                    Action::ForceQuit,
                )
                .hide(),
                InputBinding::new(KeyCode::Char('x'), Action::OpenActions),
                InputBinding::new(KeyCode::Char('?'), Action::OpenHelp),
                InputBinding::new(KeyCode::Char('f'), Action::Fullscreen),
                InputBinding::new(KeyCode::Char('r'), Action::ReloadCollection),
                InputBinding::new(KeyCode::F(2), Action::SendRequest),
                InputBinding::new(KeyCode::BackTab, Action::PreviousPane),
                InputBinding::new(KeyCode::Tab, Action::NextPane),
                InputBinding::new(KeyCode::Up, Action::Up).hide(),
                InputBinding::new(KeyCode::Down, Action::Down).hide(),
                InputBinding::new(KeyCode::Left, Action::Left).hide(),
                InputBinding::new(KeyCode::Right, Action::Right).hide(),
                InputBinding::new(KeyCode::PageUp, Action::PageUp).hide(),
                InputBinding::new(KeyCode::PageDown, Action::PageDown).hide(),
                InputBinding::new(KeyCode::Home, Action::Home).hide(),
                InputBinding::new(KeyCode::End, Action::End).hide(),
                InputBinding::new(KeyCode::Enter, Action::Submit),
                InputBinding::new(KeyCode::Esc, Action::Cancel),
            ]
            .into_iter()
            .map(|binding| (binding.action, binding))
            .collect(),
        }
    }

    /// Get a map of all available bindings
    pub fn bindings(&self) -> &IndexMap<Action, InputBinding> {
        &self.bindings
    }

    /// Get the binding associated with a particular action. Useful for mapping
    /// input in reverse, when showing available bindings to the user.
    pub fn binding(&self, action: Action) -> Option<InputBinding> {
        self.bindings.get(&action).copied()
    }

    /// Convert an input event into its bound action, if any
    pub fn action(&self, event: &Event) -> Option<Action> {
        let action = match event {
            // Trigger click on mouse *up* (feels the most natural)
            Event::Mouse(MouseEvent { kind, .. }) => match kind {
                MouseEventKind::Up(MouseButton::Left) => {
                    Some(Action::LeftClick)
                }
                MouseEventKind::Up(MouseButton::Right) => {
                    Some(Action::RightClick)
                }
                MouseEventKind::Up(MouseButton::Middle) => None,
                MouseEventKind::ScrollDown => Some(Action::ScrollDown),
                MouseEventKind::ScrollUp => Some(Action::ScrollUp),
                _ => None,
            },

            Event::Key(
                key @ KeyEvent {
                    kind: KeyEventKind::Press,
                    ..
                },
            ) => {
                // Scan all bindings for a match
                let action = self
                    .bindings
                    .values()
                    .find(|binding| binding.matches(key))
                    .map(|binding| binding.action);

                action
            }
            _ => None,
        };

        if let Some(action) = action {
            trace!(?action, "Input action");
        }

        action
    }
}

impl Default for InputEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// An input action from the user. This is context-agnostic; the action may not
/// actually mean something in the current app context. This type is just an
/// abstraction to map all possible input events to the things we actually
/// care about handling.
///
/// The order of the variants matters! It defines the ordering used in the help
/// modal (but doesn't affect behavior).
#[derive(Copy, Clone, Debug, Display, Eq, Hash, PartialEq)]
pub enum Action {
    /// This is mapped to mouse events, so it's a bit unique. Use the
    /// associated event for button/position info
    // #[display("Click")]
    // Click(MouseEvent),
    // Mouse actions do *not* get mapped, they're hard-coded. Use the
    // associated raw event for button/position info if needed
    LeftClick,
    RightClick,
    ScrollUp,
    ScrollDown,

    /// Exit the app
    Quit,
    /// A special keybinding that short-circuits the standard view input
    /// process to force an exit. Standard shutdown will *still run*, but this
    /// input can't be consumed by any components in the view tree.
    #[display("Force Quit")]
    ForceQuit,

    /// Focus the previous pane
    #[display("Prev Pane")]
    PreviousPane,
    /// Focus the next pane
    #[display("Next Pane")]
    NextPane,

    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    Home,
    End,

    /// Do a thing. E.g. select an item in a list
    Submit,
    /// Send the active request from *any* context
    #[display("Send Request")]
    SendRequest,
    /// Force a collection reload (typically it's automatic)
    #[display("Reload")]
    ReloadCollection,
    /// Embiggen a pane
    Fullscreen,
    /// Open the actions modal
    #[display("Actions")]
    OpenActions,
    #[display("Help")]
    /// Open the help modal
    OpenHelp,
    /// Close the current modal/dialog/etc.
    Cancel,
}

/// A mapping from a key input sequence to an action. This can optionally have
/// a secondary binding.
#[derive(Copy, Clone, Debug)]
pub struct InputBinding {
    action: Action,
    input: KeyCombination,
    hidden: bool,
}

impl InputBinding {
    fn new(input: impl Into<KeyCombination>, action: Action) -> Self {
        Self {
            action,
            input: input.into(),
            hidden: false,
        }
    }

    /// Don't show this binding in help dialogs
    fn hide(mut self) -> Self {
        self.hidden = true;
        self
    }

    fn matches(&self, event: &KeyEvent) -> bool {
        self.input.matches(event)
    }

    /// Should this be visible in help dialogs?
    pub fn visible(&self) -> bool {
        !self.hidden
    }

    /// Get the action associated with this binding
    pub fn action(&self) -> Action {
        self.action
    }

    /// Get the key combination associated with this binding
    pub fn input(&self) -> KeyCombination {
        self.input
    }
}

impl Display for InputBinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Don't display secondary binding in help text
        write!(f, "{} {}", self.input, self.action)
    }
}

/// Key input sequence, which can trigger an action
#[derive(Copy, Clone, Debug)]
pub struct KeyCombination {
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
            KeyCode::F(num) => write!(f, "F{}", num),
            KeyCode::Char(' ') => write!(f, "<space>"),
            KeyCode::Char(c) => write!(f, "{c}"),
            // Punting on everything else until we need it
            _ => write!(f, "???"),
        }
    }
}

impl From<KeyCode> for KeyCombination {
    fn from(key_code: KeyCode) -> Self {
        Self {
            key_code,
            modifiers: KeyModifiers::NONE,
        }
    }
}
