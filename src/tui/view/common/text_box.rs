//! A single-line text box with callbacks

use crate::tui::{
    context::TuiContext,
    input::Action,
    view::{
        draw::Draw,
        event::{Event, EventHandler, Update, UpdateContext},
    },
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nom::AsChar;
use ratatui::{
    layout::Rect,
    text::{Masked, Text},
    widgets::Paragraph,
    Frame,
};

/// Single line text submission component
#[derive(derive_more::Debug)]
pub struct TextBox {
    // Parameters
    sensitive: bool,
    focused: bool,

    state: TextState,

    // Callbacks
    #[debug(skip)]
    on_submit: Option<Callback>,
    #[debug(skip)]
    on_cancel: Option<Callback>,
}

type Callback = Box<dyn Fn(&TextBox, &mut UpdateContext)>;

impl Default for TextBox {
    fn default() -> Self {
        Self {
            sensitive: false,
            focused: true,
            state: Default::default(),
            on_submit: Default::default(),
            on_cancel: Default::default(),
        }
    }
}

impl TextBox {
    /// Mark content as sensitive, to be replaced with a placeholder character
    pub fn sensitive(mut self, sensitive: bool) -> Self {
        self.sensitive = sensitive;
        self
    }

    /// Set the callback to be called when the user hits escape
    pub fn on_cancel(
        mut self,
        on_cancel: impl 'static + Fn(&Self, &mut UpdateContext),
    ) -> Self {
        self.on_cancel = Some(Box::new(on_cancel));
        self
    }

    /// Set the callback to be called when the user hits enter
    pub fn on_submit(
        mut self,
        on_submit: impl 'static + Fn(&Self, &mut UpdateContext),
    ) -> Self {
        self.on_submit = Some(Box::new(on_submit));
        self
    }

    /// Style this text box to look active
    pub fn focus(&mut self) {
        self.focused = true;
    }

    /// Style this text box to look inactive
    pub fn unfocus(&mut self) {
        self.focused = false;
    }

    /// Move the text out of this text box and return it
    pub fn into_text(self) -> String {
        self.state.text
    }

    /// Call parent's submissionc callback
    fn submit(&mut self, context: &mut UpdateContext) {
        if let Some(on_submit) = &self.on_submit {
            on_submit(self, context);
        }
        self.unfocus();
    }

    /// Call parent's cancel callback
    fn cancel(&mut self, context: &mut UpdateContext) {
        if let Some(on_cancel) = &self.on_cancel {
            on_cancel(self, context);
        }
        self.unfocus();
    }

    /// Handle input key event to modify state
    fn handle_key_event(&mut self, key_event: KeyEvent) {
        match key_event.code {
            KeyCode::Char(c) => self.state.insert(c),
            KeyCode::Backspace => self.state.delete_left(),
            KeyCode::Delete => self.state.delete_right(),
            KeyCode::Left => {
                if key_event.modifiers.contains(KeyModifiers::CONTROL) {
                    self.state.home();
                } else {
                    self.state.left();
                }
            }
            KeyCode::Right => {
                if key_event.modifiers.contains(KeyModifiers::CONTROL) {
                    self.state.end();
                } else {
                    self.state.right();
                }
            }
            KeyCode::Home => self.state.home(),
            KeyCode::End => self.state.end(),
            _ => {}
        }
    }
}

impl EventHandler for TextBox {
    fn update(&mut self, context: &mut UpdateContext, event: Event) -> Update {
        match event {
            Event::Input {
                action: Some(Action::Submit),
                ..
            } => self.submit(context),
            Event::Input {
                action: Some(Action::Cancel),
                ..
            } => self.cancel(context),
            Event::Input {
                action: Some(Action::LeftClick),
                ..
            } => self.focus(),
            Event::Input {
                event: crossterm::event::Event::Key(key_event),
                ..
            } => self.handle_key_event(key_event),
            _ => return Update::Propagate(event),
        }
        Update::Consumed
    }
}

impl Draw for TextBox {
    fn draw(&self, frame: &mut Frame, _: (), area: Rect) {
        let theme = &TuiContext::get().theme;

        // Hide top secret data
        let text: Text = if self.sensitive {
            Masked::new(&self.state.text, '•').into()
        } else {
            self.state.text.as_str().into()
        };

        frame.render_widget(
            Paragraph::new(text).style(theme.text_box_text),
            area,
        );

        // Apply cursor styling on type
        let cursor_area = Rect {
            x: area.x + self.state.cursor_offset() as u16,
            y: area.y,
            width: 1,
            height: 1,
        };
        frame
            .buffer_mut()
            .set_style(cursor_area, theme.text_box_cursor);
    }
}

/// Encapsulation of text/cursor state. Encapsulating this makes reading and
/// testing the functionality easier.
#[derive(Debug, Default)]
struct TextState {
    text: String,
    /// **Byte** (not character) index in the text. Must be in the range `[0,
    /// text.len()]`. This must always fall on a character boundary.
    cursor: usize,
}

impl TextState {
    /// Is the cursor at the beginning of the text?
    fn is_at_home(&self) -> bool {
        self.cursor == 0
    }

    /// Is the cursor at the end of the text?
    fn is_at_end(&self) -> bool {
        self.cursor == self.char_len()
    }

    /// Get the number of **characters* (not bytes) in the text
    fn char_len(&self) -> usize {
        self.text.chars().count()
    }

    /// Move cursor to the beginning of text
    fn home(&mut self) {
        self.cursor = 0;
    }

    /// Move cursor to the end of text
    fn end(&mut self) {
        self.cursor = self.text.len();
    }

    /// Insert one character at the current cursor position
    fn insert(&mut self, c: char) {
        self.text.insert(self.cursor, c);
        self.cursor += c.len();
    }

    /// Move cursor left one **character**. This may be multiple bytes, if the
    /// character to the left is multiple bytes.
    fn left(&mut self) {
        if !self.is_at_home() {
            // unstable: use floor_char_boundary
            // https://github.com/rust-lang/rust/issues/93743
            // We know there's a char to the left, but we don't know how long
            // it is. Keep jumping left until we've hit a char boundary
            self.cursor -= 1;
            while !self.text.is_char_boundary(self.cursor) {
                self.cursor -= 1;
            }
        }
    }

    /// Move cursor right one character
    fn right(&mut self) {
        if !self.is_at_end() {
            // unstable: use ceil_char_boundary
            // https://github.com/rust-lang/rust/issues/93743
            // We checked that we're not at the end of a string, and we know the
            // cursor must be on a char boundary, so jump by the length of the
            // next char
            let next_char = self.text[self.cursor..]
                .chars()
                .next()
                .expect("Another char (not at end of string yet)");
            self.cursor += next_char.len();
        }
    }

    /// Delete character immediately left of the cursor
    fn delete_left(&mut self) {
        if !self.is_at_home() {
            self.left();
            self.text.remove(self.cursor);
        }
    }

    /// Delete character immediately rightof the cursor
    fn delete_right(&mut self) {
        if !self.is_at_end() {
            self.text.remove(self.cursor);
        }
    }

    /// Get the **character** offset of the cursor into the text
    fn cursor_offset(&self) -> usize {
        self.text[..self.cursor].chars().count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_insert() {
        let mut state = TextState::default();
        state.insert('a');
        state.insert('b');
        state.left();
        state.insert('c');
        assert_eq!(state.text, "acb");

        state.home();
        state.insert('h');
        state.end();
        state.insert('e');
        assert_eq!(state.text, "hacbe")
    }

    #[test]
    fn test_delete() {
        let mut state = TextState {
            text: "abcde".into(),
            cursor: 0,
        };

        // does nothing
        state.delete_left();
        assert_eq!(state.text, "abcde");

        state.delete_right();
        assert_eq!(state.text, "bcde");

        state.right();
        state.delete_left();
        assert_eq!(state.text, "cde");

        // does nothing
        state.end();
        state.delete_right();
        assert_eq!(state.text, "cde");

        state.delete_left();
        assert_eq!(state.text, "cd");
    }

    #[test]
    fn test_multi_char() {
        let mut state = TextState {
            text: "äëõß".into(),
            cursor: 0,
        };
        state.delete_right();
        state.end();
        state.delete_left();
        assert_eq!(state.text, "ëõ");

        state.left();
        state.insert('ü');
        assert_eq!(state.text, "ëüõ");

        assert_eq!(state.cursor_offset(), 2);
    }
}
