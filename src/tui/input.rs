//! Logic related to input handling. This is considered part of the controller.

use anyhow::bail;
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton,
    MouseEvent, MouseEventKind,
};
use derive_more::Display;
use indexmap::{indexmap, IndexMap};
use serde::Deserialize;
use std::{
    fmt::{self, Debug},
    str::FromStr,
};
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
    pub fn new(user_bindings: IndexMap<Action, InputBinding>) -> Self {
        let mut new = Self::default();
        // User bindings should overwrite any default ones
        new.bindings.extend(user_bindings);
        new
    }

    /// Get a map of all available bindings
    pub fn bindings(&self) -> &IndexMap<Action, InputBinding> {
        &self.bindings
    }

    /// Get the binding associated with a particular action. Useful for mapping
    /// input in reverse, when showing available bindings to the user.
    pub fn binding(&self, action: Action) -> Option<&InputBinding> {
        self.bindings.get(&action)
    }

    /// Append a hotkey hint to a label. If the given action is bound, adding
    /// a hint to the end of the given label. If unbound, return the label
    /// alone.
    pub fn add_hint(&self, label: impl Display, action: Action) -> String {
        if let Some(binding) = self.binding(action) {
            format!("{} ({})", label, binding)
        } else {
            label.to_string()
        }
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
                MouseEventKind::ScrollLeft => Some(Action::ScrollLeft),
                MouseEventKind::ScrollRight => Some(Action::ScrollRight),
                _ => None,
            },

            Event::Key(
                key @ KeyEvent {
                    kind: KeyEventKind::Press,
                    ..
                },
            ) => {
                // Scan all bindings for a match
                self.bindings
                    .iter()
                    .find(|(_, binding)| binding.matches(key))
                    .map(|(action, _)| *action)
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
        Self {
            bindings: indexmap! {
                // vvvvv If making changes, make sure to update the docs vvvvv
                Action::Quit => KeyCode::Char('q').into(),
                Action::ForceQuit => KeyCombination {
                    code: KeyCode::Char('c'),
                    modifiers: KeyModifiers::CONTROL,
                }.into(),
                Action::ScrollLeft => KeyCombination {
                    code: KeyCode::Left,
                    modifiers: KeyModifiers::SHIFT,
                }.into(),
                Action::ScrollRight => KeyCombination {
                    code: KeyCode::Right,
                    modifiers: KeyModifiers::SHIFT,
                }.into(),
                Action::OpenActions => KeyCode::Char('x').into(),
                Action::OpenHelp => KeyCode::Char('?').into(),
                Action::Fullscreen => KeyCode::Char('f').into(),
                Action::ReloadCollection => KeyCode::F(5).into(),
                Action::Search => KeyCode::Char('/').into(),
                Action::PreviousPane => KeyCode::BackTab.into(),
                Action::NextPane => KeyCode::Tab.into(),
                Action::Up => KeyCode::Up.into(),
                Action::Down => KeyCode::Down.into(),
                Action::Left => KeyCode::Left.into(),
                Action::Right => KeyCode::Right.into(),
                Action::PageUp => KeyCode::PageUp.into(),
                Action::PageDown => KeyCode::PageDown.into(),
                Action::Home => KeyCode::Home.into(),
                Action::End => KeyCode::End.into(),
                Action::Submit => KeyCode::Enter.into(),
                Action::Cancel => KeyCode::Esc.into(),
                Action::SelectProfileList => KeyCode::Char('p').into(),
                Action::SelectRecipeList => KeyCode::Char('l').into(),
                Action::SelectRecipe => KeyCode::Char('c').into(),
                Action::SelectRequest => KeyCode::Char('r').into(),
                Action::SelectResponse => KeyCode::Char('s').into(),
                // ^^^^^ If making changes, make sure to update the docs ^^^^^
            },
        }
    }
}

/// An input action from the user. This is context-agnostic; the action may not
/// actually mean something in the current app context. This type is just an
/// abstraction to map all possible input events to the things we actually
/// care about handling.
///
/// The order of the variants matters! It defines the ordering used in the help
/// modal (but doesn't affect behavior).
#[derive(Copy, Clone, Debug, Display, Eq, PartialEq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    // vvvvv If adding a variant, make sure to update the docs vvvvv
    //
    // Mouse actions do *not* get mapped, they're hard-coded. Use the
    // associated raw event for button/position info if needed
    LeftClick,
    RightClick,
    ScrollUp,
    ScrollDown,
    /// This can be triggered by mouse event OR key event
    #[display("Scroll Left")]
    ScrollLeft,
    /// This can be triggered by mouse event OR key event
    #[display("Scroll Right")]
    ScrollRight,

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

    /// Do a thing, e.g. submit a modal. Alternatively, send a request
    #[display("Send Request/Submit")]
    Submit,
    /// Close the current modal/dialog/etc.
    Cancel,
    /// Start a search/filter operation
    #[display("Search/Filter")]
    Search,
    /// Force a collection reload (typically it's automatic)
    #[display("Reload Collection")]
    ReloadCollection,
    /// Embiggen a pane
    Fullscreen,
    /// Open the actions modal
    #[display("Actions")]
    OpenActions,
    #[display("Help")]
    /// Open the help modal
    OpenHelp,
    /// Select profile list pane
    SelectProfileList,
    /// Select recipe list pane
    SelectRecipeList,
    /// Select recipe pane
    SelectRecipe,
    /// Select request pane
    SelectRequest,
    /// Select response pane
    SelectResponse,
    //
    // ^^^^^ If making changes, make sure to update the docs ^^^^^
}

impl Action {
    /// Should this code be shown in the help dialog?
    pub fn visible(self) -> bool {
        match self {
            // These actions are either obvious or have inline hints
            Action::ForceQuit
            | Action::Up
            | Action::Down
            | Action::Left
            | Action::Right
            | Action::PageUp
            | Action::PageDown
            | Action::Home
            | Action::End
            | Action::SelectProfileList
            | Action::SelectRecipeList
            | Action::SelectRecipe
            | Action::SelectRequest
            | Action::SelectResponse => false,
            // Most actions should not be hidden
            _ => true,
        }
    }
}

/// One or more key combinations, which should correspond to a single action
#[derive(Clone, Debug, Deserialize)]
#[cfg_attr(test, derive(PartialEq))]
#[serde(transparent)]
pub struct InputBinding(Vec<KeyCombination>);

impl InputBinding {
    /// Does a key event contain this key combo?
    fn matches(&self, event: &KeyEvent) -> bool {
        self.0.iter().any(|combo| combo.matches(event))
    }
}

impl Display for InputBinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, combo) in self.0.iter().enumerate() {
            if i > 0 {
                write!(f, ",")?;
            }
            write!(f, "{}", combo)?;
        }
        Ok(())
    }
}

impl From<KeyCombination> for InputBinding {
    fn from(combo: KeyCombination) -> Self {
        Self(vec![combo])
    }
}

impl From<KeyCode> for InputBinding {
    fn from(key_code: KeyCode) -> Self {
        KeyCombination::from(key_code).into()
    }
}

/// Key input sequence, which can trigger an action
#[derive(Copy, Clone, Debug, Deserialize)]
#[cfg_attr(test, derive(PartialEq))]
#[serde(try_from = "String")]
pub struct KeyCombination {
    code: KeyCode,
    modifiers: KeyModifiers,
}

impl KeyCombination {
    /// Char between modifiers and key codes
    const SEPARATOR: char = ' ';

    fn matches(self, event: &KeyEvent) -> bool {
        event.code == self.code && event.modifiers.contains(self.modifiers)
    }
}

impl Display for KeyCombination {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Write modifiers first
        for (name, _) in self.modifiers.iter_names() {
            write!(f, "{}{}", name.to_lowercase(), Self::SEPARATOR)?;
        }

        // Write base code
        match self.code {
            KeyCode::BackTab => write!(f, "<shift{}tab>", Self::SEPARATOR),
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
            code: key_code,
            modifiers: KeyModifiers::NONE,
        }
    }
}

impl FromStr for KeyCombination {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Last char should be the primary one, everything before should be
        // modifiers. Extra whitespace is probably a mistake, ignore it.
        let mut tokens = s.trim().split(Self::SEPARATOR);
        let code = tokens.next_back().expect("split always returns 1+ items");
        let mut code: KeyCode = parse_key_code(code)?;

        // Parse modifiers, left-to-right
        let mut modifiers = KeyModifiers::NONE;
        for modifier in tokens {
            let modifier = parse_modifier(modifier)?;
            // Prevent duplicate
            if modifiers.contains(modifier) {
                bail!("Duplicate modifier {modifier:?}");
            }
            modifiers |= modifier;
        }

        // Special case - crossterm treats shift+tab as backtab, translate it
        // automatically for the user
        if code == KeyCode::Tab && modifiers.contains(KeyModifiers::SHIFT) {
            code = KeyCode::BackTab;
            modifiers -= KeyModifiers::SHIFT;
        }

        Ok(Self { code, modifiers })
    }
}

impl TryFrom<String> for KeyCombination {
    type Error = anyhow::Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

/// Parse a plain key code
fn parse_key_code(s: &str) -> anyhow::Result<KeyCode> {
    // Check for plain char code
    if let Ok(c) = s.parse::<char>() {
        return Ok(KeyCode::Char(c));
    }
    let code = match s {
        // vvvvv If making changes, make sure to update the docs vvvvv
        "escape" | "esc" => KeyCode::Esc,
        "enter" => KeyCode::Enter,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" | "pgup" => KeyCode::PageUp,
        "pagedown" | "pgdn" => KeyCode::PageDown,
        "capslock" | "caps" => KeyCode::CapsLock,
        "tab" => KeyCode::Tab,
        "backtab" => KeyCode::BackTab,
        "backspace" => KeyCode::Backspace,
        "delete" | "del" => KeyCode::Delete,
        "insert" | "ins" => KeyCode::Insert,
        "f1" => KeyCode::F(1),
        "f2" => KeyCode::F(2),
        "f3" => KeyCode::F(3),
        "f4" => KeyCode::F(4),
        "f5" => KeyCode::F(5),
        "f6" => KeyCode::F(6),
        "f7" => KeyCode::F(7),
        "f8" => KeyCode::F(8),
        "f9" => KeyCode::F(9),
        "f10" => KeyCode::F(10),
        "f11" => KeyCode::F(11),
        "f12" => KeyCode::F(12),
        "space" => KeyCode::Char(' '),
        _ => bail!("Invalid key code {s:?}"),
        // ^^^^^ If making changes, make sure to update the docs ^^^^^
    };
    Ok(code)
}

/// Parse a key modifier
fn parse_modifier(s: &str) -> anyhow::Result<KeyModifiers> {
    let modifier = match s {
        "shift" => KeyModifiers::SHIFT,
        "alt" => KeyModifiers::ALT,
        "ctrl" => KeyModifiers::CONTROL,
        "super" => KeyModifiers::SUPER,
        "hyper" => KeyModifiers::HYPER,
        "meta" => KeyModifiers::META,
        _ => bail!("Invalid key modifier {s:?}"),
    };
    Ok(modifier)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::assert_err;
    use serde_test::{assert_de_tokens, assert_de_tokens_error, Token};

    #[test]
    fn test_parse_key_combination() {
        fn parse(s: &str) -> anyhow::Result<KeyCombination> {
            s.parse::<KeyCombination>()
        }

        fn parse_ok(s: &str) -> KeyCombination {
            parse(s).unwrap()
        }

        assert_eq!(parse_ok(" w "), KeyCode::Char('w').into());
        assert_eq!(parse_ok("f2"), KeyCode::F(2).into());
        assert_eq!(parse_ok("tab"), KeyCode::Tab.into());
        assert_eq!(parse_ok("backtab"), KeyCode::BackTab.into());
        // crossterm treats shift+tab as a special case, we translate for
        // convenience
        assert_eq!(parse_ok("shift tab"), KeyCode::BackTab.into());
        assert_eq!(
            parse_ok("alt shift tab"),
            KeyCombination {
                code: KeyCode::BackTab,
                modifiers: KeyModifiers::ALT
            }
        );
        assert_eq!(parse_ok("pgup"), KeyCode::PageUp.into());
        assert_eq!(parse_ok("pgdn"), KeyCode::PageDown.into());
        assert_eq!(parse_ok("capslock"), KeyCode::CapsLock.into());
        assert_eq!(
            parse_ok("shift f2"),
            KeyCombination {
                code: KeyCode::F(2),
                modifiers: KeyModifiers::SHIFT,
            }
        );
        assert_eq!(
            parse_ok("super hyper meta alt ctrl shift f2"),
            KeyCombination {
                code: KeyCode::F(2),
                modifiers: KeyModifiers::all(),
            }
        );

        assert_err!(parse(""), "Invalid key code");
        assert_err!(parse("  "), "Invalid key code");
        assert_err!(parse("shift+w"), "Invalid key code");
        assert_err!(parse("w shift"), "Invalid key code");
        assert_err!(parse("shart w"), "Invalid key modifier \"shart\"");
        assert_err!(parse("shift"), "Invalid key code \"shift\"");
        assert_err!(parse("alt alt w"), "Duplicate modifier");
    }

    /// Test that errors are forward correctly through deserialization, and
    /// that string/lists are both supported
    #[test]
    fn test_deserialize_input_binding() {
        assert_de_tokens(
            &InputBinding(vec![KeyCode::F(2).into(), KeyCode::F(3).into()]),
            &[
                Token::Seq { len: Some(2) },
                Token::Str("f2"),
                Token::Str("f3"),
                Token::SeqEnd,
            ],
        );

        assert_de_tokens_error::<InputBinding>(
            &[Token::Seq { len: Some(1) }, Token::Str("no"), Token::SeqEnd],
            "Invalid key code \"no\"",
        );
        assert_de_tokens_error::<InputBinding>(
            &[
                Token::Seq { len: Some(1) },
                Token::Str("shart f2"),
                Token::SeqEnd,
            ],
            "Invalid key modifier \"shart\"",
        );
        assert_de_tokens_error::<InputBinding>(
            &[
                Token::Seq { len: Some(2) },
                Token::Str("f2"),
                Token::Str("cortl f3"),
                Token::SeqEnd,
            ],
            "Invalid key modifier \"cortl\"",
        );
        assert_de_tokens_error::<InputBinding>(
            &[Token::Str("f3")],
            "invalid type: string \"f3\", expected a sequence",
        );
        assert_de_tokens_error::<InputBinding>(
            &[Token::I64(3)],
            "invalid type: integer `3`, expected a sequence",
        );
    }
}
