//! Logic related to input handling. This is considered part of the controller.

use derive_more::{Deref, Display};
use indexmap::{IndexMap, indexmap};
use itertools::Itertools;
use serde::{
    Deserialize, Serialize,
    de::{self, value::StringDeserializer},
};
use slumber_util::{
    Mapping, NEW_ISSUE_LINK,
    yaml::{
        self, DeserializeYaml, Expected, LocatedError, SourceMap, SourcedYaml,
    },
};
use std::{
    borrow::Cow,
    fmt::{self, Debug},
    iter,
    str::FromStr,
};
use terminput::{KeyCode, KeyEvent, KeyModifiers, MediaKeyCode};
use thiserror::Error;

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
    (KeyModifiers::CTRL, &["ctrl"]),
    (KeyModifiers::SUPER, &["super"]),
    (KeyModifiers::HYPER, &["hyper"]),
    (KeyModifiers::META, &["meta"]),
    // ^^^^^ If making changes, make sure to update the docs ^^^^^
]);

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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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

    /// Do a thing, e.g. submit in a text prompt. Alternatively, send a request
    #[display("Send Request/Submit")]
    Submit,
    /// Toggle checkbox and similar components on/off
    Toggle,
    /// Close the current modal/dialog/etc. OR cancel a request
    Cancel,
    /// Delete the selected object (e.g. a request)
    Delete,
    /// Trigger the workflow to provide a temporary override for a recipe value
    /// (body/param/etc.)
    Edit,
    /// Reset temporary recipe override to its default value
    Reset,
    /// Open content in the configured external pager
    View,
    /// Browse request history
    History,
    /// Start a search/filter operation
    #[display("Search/Filter")]
    Search,
    /// Enter a command to export data
    #[display("Export")]
    Export,
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
    /// Open collection selection modal (unbound by default)
    #[display("Select Collection")]
    SelectCollection,
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

impl DeserializeYaml for Action {
    fn expected() -> Expected {
        Expected::String
    }

    fn deserialize(
        yaml: SourcedYaml,
        _source_map: &SourceMap,
    ) -> yaml::Result<Self> {
        let location = yaml.location;
        let s = yaml.try_into_string()?;
        // Use serde's implementation for consistency with serialization
        <Self as Deserialize>::deserialize(StringDeserializer::new(s)).map_err(
            |error: de::value::Error| LocatedError::other(error, location),
        )
    }
}

/// One or more key combinations, which should correspond to a single action
#[derive(Clone, Debug, Serialize)]
#[cfg_attr(test, derive(PartialEq))]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(transparent)]
pub struct InputBinding(Vec<KeyCombination>);

impl InputBinding {
    /// Does this binding have no actions? If true, it should be thrown away
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Does a key event contain this key combo?
    pub fn matches(&self, event: &KeyEvent) -> bool {
        self.0.iter().any(|combo| combo.matches(event))
    }
}

impl Display for InputBinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, combo) in self.0.iter().enumerate() {
            if i > 0 {
                write!(f, ",")?;
            }
            write!(f, "{combo}")?;
        }
        Ok(())
    }
}

impl DeserializeYaml for InputBinding {
    fn expected() -> Expected {
        Expected::Sequence
    }

    fn deserialize(
        yaml: SourcedYaml,
        source_map: &SourceMap,
    ) -> yaml::Result<Self> {
        // Deserialize a list of key combinations
        DeserializeYaml::deserialize(yaml, source_map).map(Self)
    }
}

impl From<Vec<KeyCombination>> for InputBinding {
    fn from(combo: Vec<KeyCombination>) -> Self {
        Self(combo)
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
#[derive(Copy, Clone, Debug, Serialize)]
#[cfg_attr(test, derive(PartialEq))]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(into = "String", try_from = "String")]
pub struct KeyCombination {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

impl KeyCombination {
    /// Char between modifiers and key codes
    const SEPARATOR: char = ' ';

    pub fn matches(self, event: &KeyEvent) -> bool {
        // For char codes, terminal may report the code as caps
        fn to_lowercase(code: KeyCode) -> KeyCode {
            if let KeyCode::Char(c) = code {
                KeyCode::Char(c.to_ascii_lowercase())
            } else {
                code
            }
        }

        to_lowercase(event.code) == to_lowercase(self.code)
            && event.modifiers == self.modifiers
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
            KeyCode::Tab => write!(f, "<tab>"),
            KeyCode::Up => write!(f, "↑"),
            KeyCode::Down => write!(f, "↓"),
            KeyCode::Left => write!(f, "←"),
            KeyCode::Right => write!(f, "→"),
            KeyCode::Esc => write!(f, "<esc>"),
            KeyCode::Enter => write!(f, "<enter>"),
            KeyCode::Delete => write!(f, "<del>"),
            KeyCode::F(num) => write!(f, "F{num}"),
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
    type Err = InputParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Last char should be the primary one, everything before should be
        // modifiers. Ignore extra whitespace on the ends *or* the middle.
        // Filtering out empty elements is easier than building a regex to split
        let mut tokens =
            s.trim().split(Self::SEPARATOR).filter(|s| !s.is_empty());
        let code = tokens.next_back().ok_or(InputParseError::Empty)?;
        let mut modifiers = KeyModifiers::NONE;
        // `backtab` is what crossterm calls `shift tab`. We supported it in the
        // past because this used to map directly to crossterm. Keeping this
        // mapping for backward compatibility. We need snowflake logic because
        // it's a code that maps to a code+modifier
        let code: KeyCode = if code == "backtab" {
            modifiers |= KeyModifiers::SHIFT;
            KeyCode::Tab
        } else {
            parse_key_code(code)?
        };

        // Parse modifiers, left-to-right
        for modifier in tokens {
            let modifier = parse_key_modifier(modifier)?;
            // Prevent duplicate
            if modifiers.contains(modifier) {
                return Err(InputParseError::DuplicateModifier { modifier });
            }
            modifiers |= modifier;
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

/// Deserialize via FromStr
impl DeserializeYaml for KeyCombination {
    fn expected() -> Expected {
        Expected::String
    }

    fn deserialize(
        yaml: SourcedYaml,
        _source_map: &SourceMap,
    ) -> yaml::Result<Self> {
        let location = yaml.location;
        let s = yaml.try_into_string()?;
        s.parse()
            .map_err(|error| LocatedError::other(error, location))
    }
}

/// Mapping of actions to input bindings
///
/// Intuitively this should be binding:action since we get key events from the
/// user and need to look up the corresponding actions. But we can't look up a
/// binding from the map based on an input event because event<=>binding
/// matching is more nuanced that simple equality (e.g. bonus modifiers keys can
/// be ignored). We have to iterate over map when checking inputs, but keying by
/// action at least allows us to look up action=>binding for help text.
#[derive(Clone, Debug, Deref, Serialize)]
#[cfg_attr(test, derive(PartialEq))]
#[serde(transparent)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct InputMap(IndexMap<Action, InputBinding>);

impl InputMap {
    fn new(user_bindings: IndexMap<Action, InputBinding>) -> Self {
        let mut new = Self::default();
        // User bindings should overwrite any default ones
        new.0.extend(user_bindings);
        // If the user overwrote an action with an empty binding, remove it from
        // the map. This has to be done *after* the extend, so the default
        // binding is also dropped
        new.0.retain(|_, binding| !binding.is_empty());
        new
    }
}

impl Default for InputMap {
    /// Default input bindings
    fn default() -> Self {
        Self(indexmap! {
            // vvvvv If making changes, make sure to update the docs vvvvv
            Action::Quit => KeyCode::Char('q').into(),
            Action::ForceQuit => KeyCombination {
                code: KeyCode::Char('c'),
                modifiers: KeyModifiers::CTRL,
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
            Action::Export => KeyCode::Char(':').into(),
            Action::PreviousPane => KeyCombination {
                code: KeyCode::Tab,
                modifiers: KeyModifiers::SHIFT,
            }.into(),
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
            Action::Toggle => KeyCode::Char(' ').into(),
            Action::Cancel => KeyCode::Esc.into(),
            Action::Delete => KeyCode::Delete.into(),
            Action::Edit => KeyCode::Char('e').into(),
            Action::Reset => KeyCode::Char('z').into(),
            Action::View => KeyCode::Char('v').into(),
            Action::SelectCollection => KeyCode::F(3).into(),
            Action::SelectProfileList => KeyCode::Char('p').into(),
            Action::SelectRecipeList => KeyCode::Char('l').into(),
            Action::SelectRecipe => KeyCode::Char('c').into(),
            Action::SelectResponse => KeyCode::Char('r').into(),
            // ^^^^^ If making changes, make sure to update the docs ^^^^^
        })
    }
}

impl DeserializeYaml for InputMap {
    fn expected() -> Expected {
        Expected::Mapping
    }

    /// Deserialize an input map. First we deserialize the user's provided
    /// bindings, then we'll populate the map with the defaults so the consumer
    /// has access to all the bindings in one place
    fn deserialize(
        yaml: SourcedYaml,
        source_map: &SourceMap,
    ) -> yaml::Result<Self> {
        let user_bindings: IndexMap<Action, InputBinding> =
            DeserializeYaml::deserialize(yaml, source_map)?;
        Ok(Self::new(user_bindings))
    }
}

/// Error parsing input combination
#[derive(Debug, Error)]
pub enum InputParseError {
    /// Combination contains the same modifier twice
    #[error("Duplicate modifier {modifier:?}")]
    DuplicateModifier { modifier: KeyModifiers },

    /// Input is empty
    #[error("Empty key combination")]
    Empty,

    /// Key code doesn't match any known keys
    #[error(
        "Invalid key code {input:?}; key combinations should be space-separated"
    )]
    InvalidKeyCode { input: String },

    /// Key modifier doesn't match any known modifiers
    #[error(
        "Invalid key modifier {input:?}; must be one of {:?}",
        KEY_MODIFIERS.all_strings().collect_vec(),
    )]
    InvalidKeyModifier { input: String },
}

/// Parse a plain key code
fn parse_key_code(s: &str) -> Result<KeyCode, InputParseError> {
    // Check for plain char code
    if let Ok(c) = s.parse::<char>() {
        Ok(KeyCode::Char(c))
    } else {
        // Don't include the full list of options in the error message, too long
        KEY_CODES
            .get(s)
            .ok_or_else(|| InputParseError::InvalidKeyCode {
                input: s.to_owned(),
            })
    }
}

/// Convert key code to string. Inverse of parsing
fn stringify_key_code(code: KeyCode) -> Cow<'static, str> {
    if let Some(label) = KEY_CODES.get_label(code) {
        // If it's mapped, use the mapped label
        label.into()
    } else if let KeyCode::Char(c) = code {
        // Otherwise we hope it's an ASCII char
        c.to_string().into()
    } else {
        // Indicates a bug: something that isn't mapped should be
        panic!(
            "Unmapped key code {code:?}; \
            this is a bug, please open an issue: {NEW_ISSUE_LINK}"
        )
    }
}

/// Parse a key modifier
fn parse_key_modifier(s: &str) -> Result<KeyModifiers, InputParseError> {
    KEY_MODIFIERS
        .get(s)
        .ok_or_else(|| InputParseError::InvalidKeyModifier {
            input: s.to_owned(),
        })
}

/// Convert key modifier to string. Inverse of parsing
fn stringify_key_modifier(modifier: KeyModifiers) -> Cow<'static, str> {
    // unwrap() is safe because all possible modifiers are mapped
    KEY_MODIFIERS.get_label(modifier).unwrap().into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;
    use slumber_util::{assert_err, yaml::deserialize_yaml};
    use terminput::{KeyEventKind, KeyEventState};

    #[rstest]
    #[case::whitespace_stripped(" w ", KeyCode::Char('w'))]
    #[case::f_key("f2", KeyCode::F(2))]
    #[case::tab("tab", KeyCode::Tab)]
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
    // Backward compatibility: crossterm translates shift+tab as a separate
    // keycode call backtab. We previously used crossterm directly in this crate
    // so we supported this
    #[case::backtab("backtab", KeyCombination {
        code: KeyCode::Tab,
        modifiers: KeyModifiers::SHIFT,
    })]
    #[case::backtab_modifiers("ctrl backtab", KeyCombination {
        code: KeyCode::Tab,
        modifiers: KeyModifiers::CTRL | KeyModifiers::SHIFT,
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

    #[rstest]
    #[case::char_only("g", KeyCode::Char('g'), KeyModifiers::NONE, true)]
    #[case::extra_modifier("g", KeyCode::Char('G'), KeyModifiers::SHIFT, false)]
    // Terminal may report the key code as caps if shift is pressed
    #[case::caps_input(
        "shift g",
        KeyCode::Char('G'),
        KeyModifiers::SHIFT,
        true
    )]
    #[case::caps_binding(
        "shift G",
        KeyCode::Char('g'),
        KeyModifiers::SHIFT,
        true
    )]
    #[case::multiple_modifiers(
        "ctrl shift end",
        KeyCode::End,
        KeyModifiers::CTRL | KeyModifiers::SHIFT,
        true,
    )]
    #[case::missing_modifier(
        "ctrl shift end",
        KeyCode::End,
        KeyModifiers::SHIFT,
        false
    )]
    fn test_key_combination_matches(
        #[case] combination: &str,
        #[case] code: KeyCode,
        #[case] modifiers: KeyModifiers,
        #[case] match_expected: bool,
    ) {
        let combination: KeyCombination = combination.parse().unwrap();
        let event = KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        };
        assert_eq!(combination.matches(&event), match_expected);
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
        assert_eq!(
            deserialize_yaml::<InputBinding>(vec!["f2", "f3"].into()).unwrap(),
            InputBinding(vec![KeyCode::F(2).into(), KeyCode::F(3).into()])
        );

        assert_err!(
            deserialize_yaml::<InputBinding>(vec!["no"].into())
                .map_err(LocatedError::into_error),
            "Invalid key code \"no\""
        );
        assert_err!(
            deserialize_yaml::<InputBinding>(vec!["shart f2"].into())
                .map_err(LocatedError::into_error),
            "Invalid key modifier \"shart\"; must be one of \
             [\"shift\", \"alt\", \"ctrl\", \"super\", \"hyper\", \"meta\"]"
        );
        assert_err!(
            deserialize_yaml::<InputBinding>(vec!["f2", "cortl f3"].into())
                .map_err(LocatedError::into_error),
            "Invalid key modifier \"cortl\"; must be one of \
            [\"shift\", \"alt\", \"ctrl\", \"super\", \"hyper\", \"meta\"]"
        );
        assert_err!(
            deserialize_yaml::<InputBinding>("f3".into())
                .map_err(LocatedError::into_error),
            "Expected sequence, received \"f3\""
        );
        assert_err!(
            deserialize_yaml::<InputBinding>(3.into())
                .map_err(LocatedError::into_error),
            "Expected sequence, received `3`"
        );
    }

    /// Test that user-provided bindings take priority
    #[rstest]
    #[case::user_binding(
        Action::Submit,
        KeyCode::Char('w'),
        KeyCode::Char('w'),
        Some(Action::Submit)
    )]
    #[case::default_not_available(
        Action::Submit,
        KeyCode::Tab,
        KeyCode::Enter,
        None
    )]
    #[case::unbound(Action::Submit, vec![], KeyCode::Enter, None)]
    fn test_user_bindings(
        #[case] action: Action,
        #[case] binding: impl Into<InputBinding>,
        #[case] pressed: KeyCode,
        #[case] expected: Option<Action>,
    ) {
        let engine = InputMap::new(indexmap! {action => binding.into()});
        let event = KeyEvent {
            code: pressed,
            kind: KeyEventKind::Press,
            modifiers: KeyModifiers::NONE,
            state: KeyEventState::empty(),
        };
        let actual = engine
            .iter()
            .find_map(|(action, binding)| {
                // Find the action mapped to the mocked event
                if binding.matches(&event) {
                    Some(action)
                } else {
                    None
                }
            })
            .copied();
        assert_eq!(actual, expected);
    }
}
