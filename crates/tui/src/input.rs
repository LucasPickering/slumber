//! Logic related to input handling. This is considered part of the controller.

use crate::message::Message;
use derive_more::Display;
use slumber_config::{Action, InputBinding, InputMap};
use terminput::{
    Event, KeyEvent, KeyEventKind, MouseButton, MouseEvent, MouseEventKind,
    ScrollDirection,
};
use tracing::trace;

/// Top-level input manager. This handles things like bindings and mapping
/// events to actions, but then the actions are actually processed by the view.
#[derive(Debug, Default)]
pub struct InputEngine {
    bindings: InputMap,
}

impl InputEngine {
    pub fn new(bindings: InputMap) -> Self {
        Self { bindings }
    }

    /// Get a map of all available bindings
    pub fn bindings(&self) -> &InputMap {
        &self.bindings
    }

    /// Get the binding associated with a particular action. Useful for mapping
    /// input in reverse, when showing available bindings to the user.
    pub fn binding(&self, action: Action) -> Option<&InputBinding> {
        self.bindings.get(&action)
    }

    /// Get the binding associated with a particular action as a string. If the
    /// action is unbound, use a placeholder string instead
    pub fn binding_display(&self, action: Action) -> String {
        self.binding(action)
            .map(InputBinding::to_string)
            .unwrap_or_else(|| "<unbound>".to_owned())
    }

    /// Append a hotkey hint to a label. If the given action is bound, adding
    /// a hint to the end of the given label. If unbound, return the label
    /// alone.
    pub fn add_hint(&self, label: impl Display, action: Action) -> String {
        if let Some(binding) = self.binding(action) {
            format!("{label} ({binding})")
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
                MouseEventKind::Scroll(ScrollDirection::Down) => {
                    Some(Action::ScrollDown)
                }
                MouseEventKind::Scroll(ScrollDirection::Up) => {
                    Some(Action::ScrollUp)
                }
                MouseEventKind::Scroll(ScrollDirection::Left) => {
                    Some(Action::ScrollLeft)
                }
                MouseEventKind::Scroll(ScrollDirection::Right) => {
                    Some(Action::ScrollRight)
                }
                _ => None,
            },

            Event::Key(key) => {
                // Scan all bindings for a match
                self.bindings
                    .iter()
                    .find(|(_, binding)| binding.matches(key))
                    .inspect(|(action, binding)| {
                        trace!(
                            event = ?key, ?action, ?binding,
                            "Matched key event to binding"
                        );
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
        if matches!(
            event,
            Event::FocusGained
                | Event::FocusLost
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
            None
        } else {
            let action = self.action(&event);
            Some(Message::Input { event, action })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;
    use slumber_util::assert_matches;
    use terminput::{KeyCode, KeyEventState, KeyModifiers};

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
        mouse_event(MouseEventKind::Scroll(ScrollDirection::Up)),
        Some(Action::ScrollUp)
    )]
    #[case::mouse_scroll_down(
        mouse_event(MouseEventKind::Scroll(ScrollDirection::Down)),
        Some(Action::ScrollDown)
    )]
    #[case::mouse_scroll_left(
        mouse_event(MouseEventKind::Scroll(ScrollDirection::Left)),
        Some(Action::ScrollLeft)
    )]
    #[case::mouse_scroll_right(
        mouse_event(MouseEventKind::Scroll(ScrollDirection::Right)),
        Some(Action::ScrollRight)
    )]
    #[case::paste(Event::Paste("hello!".into()), None)]
    fn test_to_message_handled(
        #[case] event: Event,
        #[case] expected_action: Option<Action>,
    ) {
        let engine = InputEngine::default();
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
    #[case::key_release(key_event(
        KeyEventKind::Release,
        KeyCode::Enter,
        KeyModifiers::NONE
    ))]
    #[case::mouse_down(mouse_event(MouseEventKind::Down(MouseButton::Left)))]
    #[case::mouse_drag(mouse_event(MouseEventKind::Drag(MouseButton::Left)))]
    #[case::mouse_move(mouse_event(MouseEventKind::Moved))]
    fn test_handle_event_killed(#[case] event: Event) {
        let engine = InputEngine::default();
        assert_matches!(engine.event_to_message(event), None);
    }
}
