//! Logic related to input handling. This is considered part of the controller.

use ratatui::layout::{Position, Size};
use slumber_config::{Action, InputBinding, InputMap};
use std::fmt::Display;
use terminput::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton,
    MouseEvent, MouseEventKind, ScrollDirection,
};
use tracing::trace;

/// Map of input sequences to actions
#[derive(Debug, Default)]
pub struct InputBindings {
    bindings: InputMap,
}

impl InputBindings {
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
            .map(|binding| format!("[{binding}]"))
            .unwrap_or_else(|| "<unbound>".to_owned())
    }

    /// Append a hotkey hint to a label. If the given action is bound, adding
    /// a hint to the end of the given label. If unbound, return the label
    /// alone.
    pub fn add_hint(&self, label: impl Display, action: Action) -> String {
        if let Some(binding) = self.binding(action) {
            format!("{label} [{binding}]")
        } else {
            label.to_string()
        }
    }

    /// Convert a key event into its bound action, if any
    pub fn action(&self, event: &KeyEvent) -> Option<Action> {
        // Scan all bindings for a match
        let action = self
            .bindings
            .iter()
            .find(|(_, binding)| binding.matches(event))
            .inspect(|(action, binding)| {
                trace!(
                    ?event,
                    ?action,
                    ?binding,
                    "Matched key event to binding"
                );
            })
            .map(|(action, _)| *action);

        if let Some(action) = action {
            trace!(?action, "Input action");
        }

        action
    }

    /// Given a raw input event, generate a corresponding [InputEvent]. For key
    /// events, this includes mapping to the bound action (if any). Some
    /// events should *not* be handled; these will return `None`. This could be
    /// because they're just useless and noisy, or because they actually
    /// cause bugs (e.g. double key presses).
    pub fn convert_event(&self, event: Event) -> Option<InputEvent> {
        match event {
            // Windows sends a release event that causes double triggers
            // https://github.com/LucasPickering/slumber/issues/226
            Event::Key(KeyEvent {
                kind: KeyEventKind::Release,
                ..
            }) => None,

            // Handle everything else
            Event::Key(key_event) => {
                // Check for mapped actions
                let action = self.action(&key_event);
                Some(InputEvent::Key {
                    code: key_event.code,
                    modifiers: key_event.modifiers,
                    action,
                })
            }

            // Detecting mouse UP feels the most natural
            Event::Mouse(MouseEvent {
                kind: MouseEventKind::Up(MouseButton::Left),
                row,
                column,
                ..
            }) => Some(InputEvent::Click {
                position: (column, row).into(),
            }),
            Event::Mouse(MouseEvent {
                kind: MouseEventKind::Scroll(direction),
                row,
                column,
                ..
            }) => Some(InputEvent::Scroll {
                direction,
                position: (column, row).into(),
            }),
            Event::Paste(_) => Some(InputEvent::Paste),
            Event::Resize { rows, cols } => Some(InputEvent::Resize {
                size: Size {
                    width: cols as u16,
                    height: rows as u16,
                },
            }),

            // Toss everything else
            _ => None,
        }
    }
}

/// An event triggered by input from the user. This is a simplified version of
/// [terminput::Event] that eliminates all the possible events that we don't
/// care about handling.
#[derive(Debug, PartialEq)]
pub enum InputEvent {
    /// Key pressed down or repeated
    Key {
        /// Key pressed
        code: KeyCode,
        /// Additional modifiers keys that are active
        modifiers: KeyModifiers,
        /// Mapped input action, if any. Most consumers just care about the
        /// action. The input code/modifiers are only useful to things like
        /// text boxes that need to capture all input.
        action: Option<Action>,
    },
    /// Left click
    Click { position: Position },
    /// Scroll up/down/left/right
    Scroll {
        direction: ScrollDirection,
        position: Position,
    },
    /// Pasta!!
    Paste,
    /// Terminal was resized
    Resize { size: Size },
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;
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

    /// Test keyboard input events to `convert_event`
    #[rstest]
    #[case::key_down_mapped(
        key_event(KeyEventKind::Press, KeyCode::Enter, KeyModifiers::NONE),
        Some(InputEvent::Key {
            code: KeyCode::Enter,
            modifiers: KeyModifiers::NONE,
            action: Some(Action::Submit),
        })
    )]
    #[case::key_down_unmapped(
        key_event(KeyEventKind::Press, KeyCode::Char('k'), KeyModifiers::NONE),
        Some(InputEvent::Key {
            code: KeyCode::Char('k'),
            modifiers: KeyModifiers::NONE,
            action: None,
        })
    )]
    #[case::key_down_bonus_modifiers(
        key_event(KeyEventKind::Press, KeyCode::Enter, KeyModifiers::SHIFT),
        Some(InputEvent::Key {
            code: KeyCode::Enter,
            modifiers: KeyModifiers::SHIFT,
            action: None,
        })
    )]
    #[case::key_repeat_mapped(
        key_event(KeyEventKind::Repeat, KeyCode::Enter, KeyModifiers::NONE),
        Some(InputEvent::Key {
            code: KeyCode::Enter,
            modifiers: KeyModifiers::NONE,
            action: Some(Action::Submit),
        })
    )]
    #[case::key_repeat_unmapped(
        key_event(
            KeyEventKind::Repeat,
            KeyCode::Char('k'),
            KeyModifiers::NONE
        ),
        Some(InputEvent::Key {
            code: KeyCode::Char('k'),
            modifiers: KeyModifiers::NONE,
            action: None,
        })
    )]
    #[case::mouse_up_left(
        mouse_event(MouseEventKind::Up(MouseButton::Left)),
        Some(InputEvent::Click { position: (0, 0).into() })
    )]
    #[case::mouse_scroll_up(
        mouse_event(MouseEventKind::Scroll(ScrollDirection::Up)),
        Some(InputEvent::Scroll {
            direction: ScrollDirection::Up,
            position: (0, 0).into(),
        })
    )]
    #[case::mouse_scroll_down(
        mouse_event(MouseEventKind::Scroll(ScrollDirection::Down)),
        Some(InputEvent::Scroll {
            direction: ScrollDirection::Down,
                position: (0, 0).into(),
        })
    )]
    #[case::mouse_scroll_left(
        mouse_event(MouseEventKind::Scroll(ScrollDirection::Left)),
        Some(InputEvent::Scroll {
            direction: ScrollDirection::Left,
                position: (0, 0).into(),
        })
    )]
    #[case::mouse_scroll_right(
        mouse_event(MouseEventKind::Scroll(ScrollDirection::Right)),
        Some(InputEvent::Scroll {
            direction: ScrollDirection::Right,
                position: (0, 0).into(),
        })
    )]
    #[case::paste(Event::Paste("hello!".into()), Some(InputEvent::Paste))]
    // All these events should *not* be handled
    #[case::key_release(
        key_event(KeyEventKind::Release, KeyCode::Enter, KeyModifiers::NONE),
        None
    )]
    #[case::kill_focus_gained(Event::FocusGained, None)]
    #[case::kill_focus_lost(Event::FocusLost, None)]
    #[case::key_release(
        key_event(KeyEventKind::Release, KeyCode::Enter, KeyModifiers::NONE),
        None
    )]
    #[case::mouse_down(
        mouse_event(MouseEventKind::Down(MouseButton::Left)),
        None
    )]
    #[case::mouse_drag(
        mouse_event(MouseEventKind::Drag(MouseButton::Left)),
        None
    )]
    #[case::mouse_move(mouse_event(MouseEventKind::Moved), None)]
    fn test_convert_event(
        #[case] event: Event,
        #[case] expected: Option<InputEvent>,
    ) {
        let engine = InputBindings::default();
        let actual = engine.convert_event(event.clone());
        assert_eq!(actual, expected);
    }
}
