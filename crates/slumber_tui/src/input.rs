//! Logic related to input handling. This is considered part of the controller.

use crate::message::Message;
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton,
    MouseEvent, MouseEventKind,
};
use derive_more::Display;
use indexmap::{indexmap, IndexMap};
use slumber_config::{Action, InputBinding, KeyCombination};
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
        // If the user overwrote an action with an empty binding, remove it from
        // the map. This has to be done *after* the extend, so the default
        // binding is also dropped
        new.bindings.retain(|_, binding| !binding.is_empty());
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

            Event::Key(key) => {
                // Scan all bindings for a match
                self.bindings
                    .iter()
                    .find(|(_, binding)| binding.matches(key))
                    .inspect(|(action,binding)| {
                        trace!(event = ?key, ?action, ?binding, "Matched key event to binding");
                    })
                    .map(|(action, _)| *action)
            }
            _ => None,
        };

        if let Some(action) = action {
            trace!(?action, "Input action");
        }

        action
    }

    /// Given an input event, generate a corresponding message with mapped
    /// action. Some events will *not* generate a message, because they
    /// shouldn't get handled by components. This could be because they're just
    /// useless and noisy, or because they actually cause bugs (e.g. double key
    /// presses).
    pub fn event_to_message(&self, event: Event) -> Option<Message> {
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
            Some(Message::Input { event, action })
        } else {
            None
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
                Action::Toggle => KeyCode::Char(' ').into(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEventState;
    use rstest::rstest;
    use slumber_core::assert_matches;

    /// Helper to create a key event
    fn key_event(
        kind: KeyEventKind,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> Event {
        Event::Key(KeyEvent {
            kind,
            code,
            modifiers,
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
        let engine = InputEngine::new(indexmap! {action => binding.into()});
        let actual = engine.action(&key_event(
            KeyEventKind::Press,
            pressed,
            KeyModifiers::NONE,
        ));
        assert_eq!(actual, expected);
    }

    /// Test events that should be handled get a message generated
    #[rstest]
    #[case::key_down_mapped(
        key_event(KeyEventKind::Press, KeyCode::Enter, KeyModifiers::NONE),
        Some(Action::Submit)
    )]
    #[case::key_down_unmapped(
        key_event(KeyEventKind::Press, KeyCode::Char('k'), KeyModifiers::NONE),
        None
    )]
    #[case::key_down_bonus_modifiers(
        key_event(KeyEventKind::Press, KeyCode::Enter, KeyModifiers::SHIFT),
        None
    )]
    #[case::key_repeat_mapped(
        key_event(KeyEventKind::Repeat, KeyCode::Enter, KeyModifiers::NONE),
        Some(Action::Submit)
    )]
    #[case::key_repeat_unmapped(
        key_event(
            KeyEventKind::Repeat,
            KeyCode::Char('k'),
            KeyModifiers::NONE
        ),
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
    fn test_to_message_handled(
        #[case] event: Event,
        #[case] expected_action: Option<Action>,
    ) {
        let engine = InputEngine::new(IndexMap::default());
        let (queued_event, queued_action) = assert_matches!(
            engine.event_to_message(event.clone()),
            Some(Message::Input { event, action }) => (event, action),
        );
        assert_eq!(queued_event, event);
        assert_eq!(queued_action, expected_action);
    }

    /// Test that these events get thrown out, and never queue any messages
    #[rstest]
    #[case::focus_gained(Event::FocusGained)]
    #[case::focus_lost(Event::FocusLost)]
    #[case::resize(Event::Resize(10, 10))]
    #[case::key_release(key_event(
        KeyEventKind::Release,
        KeyCode::Enter,
        KeyModifiers::NONE
    ))]
    #[case::mouse_down(mouse_event(MouseEventKind::Down(MouseButton::Left)))]
    #[case::mouse_drag(mouse_event(MouseEventKind::Drag(MouseButton::Left)))]
    #[case::mouse_move(mouse_event(MouseEventKind::Moved))]
    fn test_handle_event_killed(#[case] event: Event) {
        let engine = InputEngine::new(IndexMap::default());
        assert_matches!(engine.event_to_message(event), None);
    }
}
