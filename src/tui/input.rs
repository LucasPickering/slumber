//! Logic related to input handling. This is considered part of the controller.

use crate::{
    tui::message::{Message, MessageSender},
    util::Mapping,
};
use anyhow::{anyhow, bail};
use crossterm::event::{
    Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
    MediaKeyCode, MouseButton, MouseEvent, MouseEventKind,
};
use derive_more::Display;
use futures::StreamExt;
use indexmap::{indexmap, IndexMap};
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use std::{
    borrow::Cow,
    fmt::{self, Debug},
    iter,
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
    /// Key code to string mappings
    const KEY_CODES: Mapping<'static, KeyCode> = Mapping::new(&[
        // unstable: include ASCII chars
        // https://github.com/rust-lang/rust/issues/110998
        // vvvvv If making changes, make sure to update the docs vvvvv
        (KeyCode::Esc, &["escape", "esc"]),
        (KeyCode::Enter, &["enter"]),
        (KeyCode::Left, &["left"]),
        (KeyCode::Right, &["right"]),
        (KeyCode::Up, &["up"]),
        (KeyCode::Down, &["down"]),
        (KeyCode::Home, &["home"]),
        (KeyCode::End, &["end"]),
        (KeyCode::PageUp, &["pageup", "pgup"]),
        (KeyCode::PageDown, &["pagedown", "pgdn"]),
        (KeyCode::Tab, &["tab"]),
        (KeyCode::BackTab, &["backtab"]),
        (KeyCode::Backspace, &["backspace"]),
        (KeyCode::Delete, &["delete", "del"]),
        (KeyCode::Insert, &["insert", "ins"]),
        (KeyCode::CapsLock, &["capslock", "caps"]),
        (KeyCode::ScrollLock, &["scrolllock"]),
        (KeyCode::NumLock, &["numlock"]),
        (KeyCode::PrintScreen, &["printscreen"]),
        (KeyCode::Pause, &["pausebreak"]),
        (KeyCode::Menu, &["menu"]),
        (KeyCode::KeypadBegin, &["keypadbegin"]),
        (KeyCode::F(1), &["f1"]),
        (KeyCode::F(2), &["f2"]),
        (KeyCode::F(3), &["f3"]),
        (KeyCode::F(4), &["f4"]),
        (KeyCode::F(5), &["f5"]),
        (KeyCode::F(6), &["f6"]),
        (KeyCode::F(7), &["f7"]),
        (KeyCode::F(8), &["f8"]),
        (KeyCode::F(9), &["f9"]),
        (KeyCode::F(10), &["f10"]),
        (KeyCode::F(11), &["f11"]),
        (KeyCode::F(12), &["f12"]),
        (KeyCode::Char(' '), &["space"]),
        (KeyCode::Media(MediaKeyCode::Play), &["play"]),
        (KeyCode::Media(MediaKeyCode::Pause), &["pause"]),
        (KeyCode::Media(MediaKeyCode::PlayPause), &["playpause"]),
        (KeyCode::Media(MediaKeyCode::Reverse), &["reverse"]),
        (KeyCode::Media(MediaKeyCode::Stop), &["stop"]),
        (KeyCode::Media(MediaKeyCode::FastForward), &["fastforward"]),
        (KeyCode::Media(MediaKeyCode::Rewind), &["rewind"]),
        (KeyCode::Media(MediaKeyCode::TrackNext), &["tracknext"]),
        (
            KeyCode::Media(MediaKeyCode::TrackPrevious),
            &["trackprevious"],
        ),
        (KeyCode::Media(MediaKeyCode::Record), &["record"]),
        (KeyCode::Media(MediaKeyCode::LowerVolume), &["lowervolume"]),
        (KeyCode::Media(MediaKeyCode::RaiseVolume), &["raisevolume"]),
        (KeyCode::Media(MediaKeyCode::MuteVolume), &["mute"]),
        // ^^^^^ If making changes, make sure to update the docs ^^^^^
    ]);
    /// Key modifier to string mappings
    const KEY_MODIFIERS: Mapping<'static, KeyModifiers> = Mapping::new(&[
        // vvvvv If making changes, make sure to update the docs vvvvv
        (KeyModifiers::SHIFT, &["shift"]),
        (KeyModifiers::ALT, &["alt"]),
        (KeyModifiers::CONTROL, &["ctrl"]),
        (KeyModifiers::SUPER, &["super"]),
        (KeyModifiers::HYPER, &["hyper"]),
        (KeyModifiers::META, &["meta"]),
        // ^^^^^ If making changes, make sure to update the docs ^^^^^
    ]);

    pub fn new(user_bindings: IndexMap<Action, InputBinding>) -> Self {
        let mut new = Self::default();
        // User bindings should overwrite any default ones
        new.bindings.extend(user_bindings);
        new
    }

    /// Listen for input from the terminal, and forward relevant events to the
    /// message queue. This future will run indefinitely, so it should be
    /// spawned in its own task.
    pub async fn input_loop(&self, messages_tx: MessageSender) {
        let mut stream = EventStream::new();
        while let Some(result) = stream.next().await {
            // Failure to read input is both weird and fatal, so panic
            let event = result.expect("Error reading terminal input");
            self.handle_event(&messages_tx, event);
        }
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

            Event::Key(key) => {
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

    /// Given an input event, generate and queue a corresponding message. Some
    /// events will *not* generate a message, because they shouldn't get
    /// handled by components. This could be because they're just useless and
    /// noisy, or because they actually cause bugs (e.g. double key presses).
    fn handle_event(&self, messages_tx: &MessageSender, event: Event) {
        if !matches!(
            event,
            Event::FocusGained
                | Event::FocusLost
                | Event::Resize(_, _)
                // Windows sends a release event that causes double triggers
                // https://github.com/LucasPickering/slumber/issues/226
                | Event::Key(KeyEvent {
                    kind: KeyEventKind::Release,
                    ..
                })
                | Event::Mouse(MouseEvent {
                    kind: MouseEventKind::Down(_)
                    | MouseEventKind::Drag(_)
                    | MouseEventKind::Moved,
                    ..
                })
        ) {
            let action = self.action(&event);
            messages_tx.send(Message::Input { event, action });
        }
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
                Action::History => KeyCode::Char('h').into(),
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
                Action::SelectResponse => KeyCode::Char('r').into(),
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
#[derive(
    Copy, Clone, Debug, Display, Eq, PartialEq, Hash, Serialize, Deserialize,
)]
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
    /// Browse request history
    History,
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
    /// Select response pane
    #[serde(alias = "select_request")] // Backward compatibility
    SelectResponse,
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
            | Action::SelectResponse => false,
            // Most actions should not be hidden
            _ => true,
        }
    }
}

/// One or more key combinations, which should correspond to a single action
#[derive(Clone, Debug, Serialize, Deserialize)]
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
#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(test, derive(PartialEq))]
#[serde(into = "String", try_from = "String")]
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

/// User-friendly and compact display for a key combination. This is meant to
/// just be used in the UI, *not* for serialization!
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
        // modifiers. Ignore extra whitespace on the ends *or* the middle.
        // Filtering out empty elements is easier than building a regex to split
        let mut tokens =
            s.trim().split(Self::SEPARATOR).filter(|s| !s.is_empty());
        let code = tokens
            .next_back()
            .ok_or_else(|| anyhow!("Empty key combination"))?;
        let mut code: KeyCode = parse_key_code(code)?;

        // Parse modifiers, left-to-right
        let mut modifiers = KeyModifiers::NONE;
        for modifier in tokens {
            let modifier = parse_key_modifier(modifier)?;
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

/// For serialization
impl From<KeyCombination> for String {
    fn from(key_combo: KeyCombination) -> Self {
        key_combo
            .modifiers
            .iter()
            .map(stringify_key_modifier)
            .chain(iter::once(stringify_key_code(key_combo.code)))
            .join(" ")
    }
}

/// For deserialization
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
        Ok(KeyCode::Char(c))
    } else {
        // Don't include the full list of options in the error message, too long
        InputEngine::KEY_CODES.get(s).ok_or_else(|| {
            anyhow!(
                "Invalid key code {s:?}; key combinations should be space-separated"
            )
        })
    }
}

/// Convert key code to string. Inverse of parsing
fn stringify_key_code(code: KeyCode) -> Cow<'static, str> {
    // ASCII chars aren't in the mapping, they're handled specially
    if let KeyCode::Char(c) = code {
        c.to_string().into()
    } else {
        InputEngine::KEY_CODES.get_label(code).into()
    }
}

/// Parse a key modifier
fn parse_key_modifier(s: &str) -> anyhow::Result<KeyModifiers> {
    InputEngine::KEY_MODIFIERS.get(s).ok_or_else(|| {
        anyhow!(
            "Invalid key modifier {s:?}; must be one of {:?}",
            InputEngine::KEY_MODIFIERS.all_strings().collect_vec()
        )
    })
}

/// Convert key modifier to string. Inverse of parsing
fn stringify_key_modifier(modifier: KeyModifiers) -> Cow<'static, str> {
    InputEngine::KEY_MODIFIERS.get_label(modifier).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_util::{assert_err, assert_matches},
        tui::test_util::{harness, TestHarness},
    };
    use crossterm::event::{KeyEventState, MediaKeyCode};
    use rstest::rstest;
    use serde_test::{assert_de_tokens, assert_de_tokens_error, Token};

    /// Helper to create a key event
    fn key_event(kind: KeyEventKind, code: KeyCode) -> Event {
        Event::Key(KeyEvent {
            kind,
            code,
            modifiers: KeyModifiers::NONE,
            state: KeyEventState::empty(),
        })
    }

    /// Helper to create a mouse event
    fn mouse_event(kind: MouseEventKind) -> Event {
        Event::Mouse(MouseEvent {
            kind,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        })
    }

    /// Test that each event queues a corresponding message
    #[rstest]
    #[case::key_down_mapped(
        key_event(KeyEventKind::Press, KeyCode::Enter),
        Some(Action::Submit)
    )]
    #[case::key_down_unmapped(
        key_event(KeyEventKind::Press, KeyCode::Char('k')),
        None
    )]
    #[case::key_repeat_mapped(
        key_event(KeyEventKind::Repeat, KeyCode::Enter),
        Some(Action::Submit)
    )]
    #[case::key_repeat_unmapped(
        key_event(KeyEventKind::Repeat, KeyCode::Char('k')),
        None
    )]
    #[case::mouse_up_left(
        mouse_event(MouseEventKind::Up(MouseButton::Left)),
        Some(Action::LeftClick)
    )]
    #[case::mouse_up_right(
        mouse_event(MouseEventKind::Up(MouseButton::Right)),
        Some(Action::RightClick)
    )]
    #[case::mouse_scroll_up(
        mouse_event(MouseEventKind::ScrollUp),
        Some(Action::ScrollUp)
    )]
    #[case::mouse_scroll_down(
        mouse_event(MouseEventKind::ScrollDown),
        Some(Action::ScrollDown)
    )]
    #[case::mouse_scroll_left(
        mouse_event(MouseEventKind::ScrollLeft),
        Some(Action::ScrollLeft)
    )]
    #[case::mouse_scroll_right(
        mouse_event(MouseEventKind::ScrollRight),
        Some(Action::ScrollRight)
    )]
    #[case::paste(Event::Paste("hello!".into()), None)]
    fn test_handle_event_queued(
        mut harness: TestHarness,
        #[case] event: Event,
        #[case] expected_action: Option<Action>,
    ) {
        let engine = InputEngine::new(IndexMap::default());
        engine.handle_event(harness.messages_tx(), event.clone());
        let (queued_event, queued_action) = assert_matches!(
            harness.pop_message_now(),
            Message::Input { event, action } => (event, action),
        );
        assert_eq!(queued_event, event);
        assert_eq!(queued_action, expected_action);
    }

    /// Test that these events get thrown out, and never queue any messages
    #[rstest]
    #[case::focus_gained(Event::FocusGained)]
    #[case::focus_lost(Event::FocusLost)]
    #[case::resize(Event::Resize(10, 10))]
    #[case::key_release(key_event(KeyEventKind::Release, KeyCode::Enter))]
    #[case::mouse_down(mouse_event(MouseEventKind::Down(MouseButton::Left)))]
    #[case::mouse_drag(mouse_event(MouseEventKind::Drag(MouseButton::Left)))]
    #[case::mouse_move(mouse_event(MouseEventKind::Moved))]
    fn test_handle_event_killed(
        mut harness: TestHarness,
        #[case] event: Event,
    ) {
        let engine = InputEngine::new(IndexMap::default());
        engine.handle_event(harness.messages_tx(), event);
        harness.assert_messages_empty();
    }

    #[rstest]
    #[case::whitespace_stripped(" w ", KeyCode::Char('w'))]
    #[case::f_key("f2", KeyCode::F(2))]
    #[case::tab("tab", KeyCode::Tab)]
    #[case::backtab("backtab", KeyCode::BackTab)]
    // crossterm treats shift+tab as a special case, we translate for
    // convenience
    #[case::shift_tab("shift tab", KeyCode::BackTab)]
    #[case::multiple_modifiers("alt shift tab", KeyCombination {
        code: KeyCode::BackTab,
        modifiers: KeyModifiers::ALT
    })]
    #[case::page_up("pgup", KeyCode::PageUp)]
    #[case::page_down("pgdn", KeyCode::PageDown)]
    #[case::caps_lock("capslock", KeyCode::CapsLock)]
    #[case::f_key_with_modifier("shift f2", KeyCombination {
        code: KeyCode::F(2),
        modifiers: KeyModifiers::SHIFT,
    })]
    // Bonus spaces!
    #[case::extra_whitespace("shift  f2", KeyCombination {
        code: KeyCode::F(2),
        modifiers: KeyModifiers::SHIFT,
    })]
    #[case::extra_extra_whitespace("shift   f2", KeyCombination {
        code: KeyCode::F(2),
        modifiers: KeyModifiers::SHIFT,
    })]
    #[case::all_modifiers("super hyper meta alt ctrl shift f2", KeyCombination {
        code: KeyCode::F(2),
        modifiers: KeyModifiers::all(),
    })]
    fn test_parse_key_combination(
        #[case] input: &str,
        #[case] expected: impl Into<KeyCombination>,
    ) {
        assert_eq!(input.parse::<KeyCombination>().unwrap(), expected.into());
    }

    #[rstest]
    #[case::empty("", "Empty key combination")]
    #[case::whitespace_only("  ", "Empty key combination")]
    #[case::invalid_delimiter("shift+w", "Invalid key code")]
    #[case::modifier_last("w shift", "Invalid key code")]
    #[case::invalid_modifier("shart w", "Invalid key modifier \"shart\"")]
    #[case::modifier_only("shift", "Invalid key code \"shift\"")]
    #[case::duplicate_modifier("alt alt w", "Duplicate modifier")]
    fn test_parse_key_combination_error(
        #[case] input: &str,
        #[case] expected_error: &str,
    ) {
        assert_err!(input.parse::<KeyCombination>(), expected_error);
    }

    /// Test stringifying/parsing key codes
    #[test]
    fn test_key_code() {
        // Build an iter of all codes
        let codes = [
            KeyCode::Backspace,
            KeyCode::Enter,
            KeyCode::Left,
            KeyCode::Right,
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::Home,
            KeyCode::End,
            KeyCode::PageUp,
            KeyCode::PageDown,
            KeyCode::Tab,
            KeyCode::BackTab,
            KeyCode::Delete,
            KeyCode::Insert,
            // Intentionally omitting Null (what is it??)
            KeyCode::Esc,
            KeyCode::CapsLock,
            KeyCode::ScrollLock,
            KeyCode::NumLock,
            KeyCode::PrintScreen,
            KeyCode::Pause,
            KeyCode::Menu,
            KeyCode::KeypadBegin,
        ]
        .into_iter()
        // F keys
        .chain((1..=12).map(KeyCode::F))
        // Chars (ASCII only)
        .chain((32..=126).map(|c| KeyCode::Char(char::from_u32(c).unwrap())))
        // Media keys
        .chain(
            [
                MediaKeyCode::Play,
                MediaKeyCode::Pause,
                MediaKeyCode::PlayPause,
                MediaKeyCode::Reverse,
                MediaKeyCode::Stop,
                MediaKeyCode::FastForward,
                MediaKeyCode::Rewind,
                MediaKeyCode::TrackNext,
                MediaKeyCode::TrackPrevious,
                MediaKeyCode::Record,
                MediaKeyCode::LowerVolume,
                MediaKeyCode::RaiseVolume,
                MediaKeyCode::MuteVolume,
            ]
            .into_iter()
            .map(KeyCode::Media),
        );
        // Intentionally ignore modifier key codes, they're treated separately

        // Round trip should get us in the same spot
        for code in codes {
            let s = stringify_key_code(code);
            let parsed = parse_key_code(&s).unwrap();
            assert_eq!(code, parsed, "code parse mismatch");
        }
    }

    /// Test stringifying/parsing each key modifier
    #[test]
    fn test_key_modifier() {
        // Round trip should get us in the same spot
        for modifier in KeyModifiers::all() {
            let s = stringify_key_modifier(modifier);
            let parsed = parse_key_modifier(&s).unwrap();
            assert_eq!(modifier, parsed, "modifier parse mismatch");
        }
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
            "Invalid key code \"no\"; key combinations should be space-separated",
        );
        assert_de_tokens_error::<InputBinding>(
            &[
                Token::Seq { len: Some(1) },
                Token::Str("shart f2"),
                Token::SeqEnd,
            ],
            "Invalid key modifier \"shart\"; must be one of \
             [\"shift\", \"alt\", \"ctrl\", \"super\", \"hyper\", \"meta\"]",
        );
        assert_de_tokens_error::<InputBinding>(
            &[
                Token::Seq { len: Some(2) },
                Token::Str("f2"),
                Token::Str("cortl f3"),
                Token::SeqEnd,
            ],
            "Invalid key modifier \"cortl\"; must be one of \
            [\"shift\", \"alt\", \"ctrl\", \"super\", \"hyper\", \"meta\"]",
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
