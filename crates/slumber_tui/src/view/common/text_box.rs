//! A single-line text box with callbacks

use crate::{
    context::TuiContext,
    view::{
        draw::{Draw, DrawMetadata},
        event::{Event, EventHandler, Update},
    },
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use persisted::PersistedContainer;
use ratatui::{
    layout::Rect,
    text::{Line, Masked, Text},
    widgets::Paragraph,
    Frame,
};
use slumber_config::Action;

/// Single line text submission component
#[derive(derive_more::Debug, Default)]
pub struct TextBox {
    // Parameters
    sensitive: bool,
    placeholder_text: String,
    /// Predicate function to apply visual validation effect
    #[debug(skip)]
    validator: Option<Validator>,

    state: TextState,

    // Callbacks
    /// Called when user clicks to start editing
    #[debug(skip)]
    on_click: Option<Callback>,
    /// Called when user exits with submission (e.g. Enter)
    #[debug(skip)]
    on_submit: Option<Callback>,
    /// Called when user exits without saving (e.g. Escape)
    #[debug(skip)]
    on_cancel: Option<Callback>,
}

type Callback = Box<dyn Fn()>;

type Validator = Box<dyn Fn(&str) -> bool>;

impl TextBox {
    /// Set initialize value for the text box
    pub fn default_value(mut self, default: String) -> Self {
        self.state.text = default;
        self.state.end();
        self
    }

    /// Mark content as sensitive, to be replaced with a placeholder character
    pub fn sensitive(mut self, sensitive: bool) -> Self {
        self.sensitive = sensitive;
        self
    }

    /// Set placeholder (text to show when content is empty) on initialization
    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder_text = placeholder.into();
        self
    }

    /// Set validation function. If input is invalid, the submission callback
    /// will be blocked, meaning the user must fix the error or cancel.
    pub fn validator(
        mut self,
        validator: impl 'static + Fn(&str) -> bool,
    ) -> Self {
        self.validator = Some(Box::new(validator));
        self
    }
    /// Set the callback to be called when the user clicks the textbox
    pub fn on_click(mut self, on_click: impl 'static + Fn()) -> Self {
        self.on_click = Some(Box::new(on_click));
        self
    }

    /// Set the callback to be called when the user hits escape
    pub fn on_cancel(mut self, on_cancel: impl 'static + Fn()) -> Self {
        self.on_cancel = Some(Box::new(on_cancel));
        self
    }

    /// Set the callback to be called when the user hits enter
    pub fn on_submit(mut self, on_submit: impl 'static + Fn()) -> Self {
        self.on_submit = Some(Box::new(on_submit));
        self
    }

    /// Get current text
    pub fn text(&self) -> &str {
        &self.state.text
    }

    /// Move the text out of this text box and return it
    pub fn into_text(self) -> String {
        self.state.text
    }

    /// Set text, and move the cursor to the end
    pub fn set_text(&mut self, text: String) {
        self.state.text = text;
        self.state.end();
        self.submit();
    }

    /// Check if the current input text is valid. Always returns true if there
    /// is no validator
    fn is_valid(&self) -> bool {
        self.text().is_empty()
            || self
                .validator
                .as_ref()
                .map(|validator| validator(self.text()))
                .unwrap_or(true)
    }

    /// Call parent's submission callback
    fn submit(&mut self) {
        if self.is_valid() {
            call(&self.on_submit);
        }
    }

    /// Handle input key event to modify text/cursor state
    fn handle_key_event(&mut self, key_event: KeyEvent) {
        match key_event.code {
            // Don't handle keystrokes if the user is holding a modifier
            KeyCode::Char(c)
                if (key_event.modifiers - KeyModifiers::SHIFT).is_empty() =>
            {
                self.state.insert(c)
            }
            KeyCode::Backspace => self.state.delete_left(),
            KeyCode::Delete => self.state.delete_right(),
            KeyCode::Left => {
                if key_event.modifiers == KeyModifiers::CONTROL {
                    self.state.home();
                } else {
                    self.state.left();
                }
            }
            KeyCode::Right => {
                if key_event.modifiers == KeyModifiers::CONTROL {
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
    fn update(&mut self, event: Event) -> Update {
        match event {
            Event::Input {
                action: Some(Action::Submit),
                ..
            } => self.submit(),
            Event::Input {
                action: Some(Action::Cancel),
                ..
            } => call(&self.on_cancel),
            Event::Input {
                action: Some(Action::LeftClick),
                ..
            } => call(&self.on_click),
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
    fn draw(&self, frame: &mut Frame, _: (), metadata: DrawMetadata) {
        let styles = &TuiContext::get().styles;

        // Hide top secret data
        let text: Text = if self.state.text.is_empty() {
            Line::from(self.placeholder_text.as_str())
                .style(styles.text_box.placeholder)
                .into()
        } else if self.sensitive {
            Masked::new(&self.state.text, '•').into()
        } else {
            self.state.text.as_str().into()
        };

        // Draw the text
        let style = if self.is_valid() {
            styles.text_box.text
        } else {
            styles.text_box.invalid
        };
        frame.render_widget(Paragraph::new(text).style(style), metadata.area());

        if metadata.has_focus() {
            // Apply cursor styling on type
            let cursor_area = Rect {
                x: metadata.area().x + self.state.cursor_offset() as u16,
                y: metadata.area().y,
                width: 1,
                height: 1,
            };
            frame
                .buffer_mut()
                .set_style(cursor_area, styles.text_box.cursor);
        }
    }
}

/// Encapsulation of text/cursor state. Encapsulating this makes reading and
/// testing the functionality easier.
#[derive(Debug, Default)]
#[cfg_attr(test, derive(PartialEq))]
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
        self.cursor += c.len_utf8();
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
            self.cursor += next_char.len_utf8();
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

impl PersistedContainer for TextBox {
    type Value = String;

    fn get_persisted(&self) -> Self::Value {
        self.state.text.clone()
    }

    fn set_persisted(&mut self, value: Self::Value) {
        self.set_text(value);
    }
}

/// Call a callback if defined
fn call(f: &Option<impl Fn()>) {
    if let Some(f) = f {
        f();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_util::{harness, TestHarness},
        view::test_util::TestComponent,
    };
    use ratatui::text::Span;
    use rstest::rstest;
    use std::{cell::Cell, rc::Rc};

    /// Create a span styled as the cursor
    fn cursor(text: &str) -> Span {
        Span::styled(text, TuiContext::get().styles.text_box.cursor)
    }

    /// Create a span styled as text in the box
    fn text(text: &str) -> Span {
        Span::styled(text, TuiContext::get().styles.text_box.text)
    }

    /// Assert that text state matches text/cursor location. Cursor location is
    /// a *character* offset, not byte offset
    fn assert_state(state: &TextState, text: &str, cursor: usize) {
        assert_eq!(state.text, text, "Text does not match");
        assert_eq!(
            state.cursor_offset(),
            cursor,
            "Cursor character offset does not match"
        )
    }

    /// Helper for counting calls to a closure
    #[derive(Clone, Debug, Default)]
    struct Counter(Rc<Cell<usize>>);

    impl Counter {
        fn increment(&self) {
            self.0.set(self.0.get() + 1);
        }

        /// Create a callback that just calls the counter
        fn callback(&self) -> impl Fn() {
            let counter = self.clone();
            move || {
                counter.increment();
            }
        }
    }

    impl PartialEq<usize> for Counter {
        fn eq(&self, other: &usize) -> bool {
            self.0.get() == *other
        }
    }

    /// Test the basic interaction loop on the text box
    #[rstest]
    fn test_interaction(#[with(10, 1)] harness: TestHarness) {
        let click_count = Counter::default();
        let submit_count = Counter::default();
        let cancel_count = Counter::default();
        let mut component = TestComponent::new(
            harness,
            TextBox::default()
                .on_click(click_count.callback())
                .on_submit(submit_count.callback())
                .on_cancel(cancel_count.callback()),
            (),
        );

        // Assert initial state/view
        assert_state(&component.data().state, "", 0);
        component.assert_buffer_lines([vec![cursor(" "), text("         ")]]);

        // Type some text
        component.send_text("hello!").assert_empty();
        assert_state(&component.data().state, "hello!", 6);
        component.assert_buffer_lines([vec![
            text("hello!"),
            cursor(" "),
            text("   "),
        ]]);

        // Sending with a modifier applied should do nothing, unless it's shift
        component
            .send_key_modifiers(KeyCode::Char('W'), KeyModifiers::SHIFT)
            .assert_empty();
        assert_state(&component.data().state, "hello!W", 7);
        component
            .send_key_modifiers(
                KeyCode::Char('W'), // this is what crossterm actually sends
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            )
            .assert_empty();
        assert_state(&component.data().state, "hello!W", 7);

        // Test callbacks
        component.click(0, 0).assert_empty();
        assert_eq!(click_count, 1);

        component.send_key(KeyCode::Enter).assert_empty();
        assert_eq!(submit_count, 1);

        component.send_key(KeyCode::Esc).assert_empty();
        assert_eq!(cancel_count, 1);
    }

    /// Test text navigation and deleting. [TextState] has its own tests so
    /// we're mostly just testing that keys are mapped correctly
    #[rstest]
    fn test_navigation(#[with(10, 1)] harness: TestHarness) {
        let mut component = TestComponent::new(harness, TextBox::default(), ());

        // Type some text
        component.send_text("hello!").assert_empty();
        assert_state(&component.data().state, "hello!", 6);

        // Move around, delete some text.
        component.send_key(KeyCode::Left).assert_empty();
        assert_state(&component.data().state, "hello!", 5);

        component.send_key(KeyCode::Backspace).assert_empty();
        assert_state(&component.data().state, "hell!", 4);

        component.send_key(KeyCode::Delete).assert_empty();
        assert_state(&component.data().state, "hell", 4);

        component.send_key(KeyCode::Home).assert_empty();
        assert_state(&component.data().state, "hell", 0);

        component.send_key(KeyCode::Right).assert_empty();
        assert_state(&component.data().state, "hell", 1);

        component.send_key(KeyCode::End).assert_empty();
        assert_state(&component.data().state, "hell", 4);
    }

    #[rstest]
    fn test_sensitive(#[with(6, 1)] harness: TestHarness) {
        let mut component =
            TestComponent::new(harness, TextBox::default().sensitive(true), ());

        component.send_text("hello").assert_empty();

        assert_state(&component.data().state, "hello", 5);
        component.assert_buffer_lines([vec![text("•••••"), cursor(" ")]]);
    }

    #[rstest]
    fn test_placeholder(#[with(6, 1)] harness: TestHarness) {
        let component = TestComponent::new(
            harness,
            TextBox::default().placeholder("hello"),
            (),
        );

        assert_state(&component.data().state, "", 0);
        let styles = &TuiContext::get().styles.text_box;
        component.assert_buffer_lines([vec![
            cursor("h"),
            Span::styled("ello", styles.text.patch(styles.placeholder)),
            text(" "),
        ]]);
    }

    #[rstest]
    fn test_validator(#[with(6, 1)] harness: TestHarness) {
        let mut component = TestComponent::new(
            harness,
            TextBox::default().validator(|text| text.len() <= 2),
            (),
        );

        // Valid text, everything is normal
        component.send_text("he").assert_empty();
        component.assert_buffer_lines([vec![
            text("he"),
            cursor(" "),
            text("   "),
        ]]);

        // Invalid text, styling changes
        component.send_text("llo").assert_empty();
        component.assert_buffer_lines([vec![
            Span::styled("hello", TuiContext::get().styles.text_box.invalid),
            cursor(" "),
        ]]);
    }

    #[test]
    fn test_state_insert() {
        let mut state = TextState::default();
        state.insert('a');
        state.insert('b');
        state.left();
        state.insert('c');
        assert_state(&state, "acb", 2);

        state.home();
        state.insert('h');
        state.end();
        state.insert('e');
        assert_state(&state, "hacbe", 5);
    }

    #[test]
    fn test_state_delete() {
        let mut state = TextState {
            text: "abcde".into(),
            cursor: 0,
        };

        // does nothing
        state.delete_left();
        assert_state(&state, "abcde", 0);

        state.delete_right();
        assert_state(&state, "bcde", 0);

        state.right();
        state.delete_left();
        assert_state(&state, "cde", 0);

        // does nothing
        state.end();
        state.delete_right();
        assert_state(&state, "cde", 3);

        state.delete_left();
        assert_state(&state, "cd", 2);
    }

    /// Test characters that contain multiple bytes
    #[test]
    fn test_state_multibyte_char() {
        let mut state = TextState {
            text: "äëõß".into(),
            cursor: 0,
        };
        state.delete_right();
        state.end();
        state.delete_left();
        assert_state(&state, "ëõ", 2);

        state.left();
        state.insert('ü');
        assert_state(&state, "ëüõ", 2);
    }
}
