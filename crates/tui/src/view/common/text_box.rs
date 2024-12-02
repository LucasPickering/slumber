//! A single-line text box with callbacks

use crate::{
    context::TuiContext,
    message::Message,
    view::{
        context::UpdateContext,
        draw::{Draw, DrawMetadata},
        event::{Event, EventHandler, Update},
        util::Debounce,
        ViewContext,
    },
};
use crossterm::event::{KeyCode, KeyModifiers};
use persisted::PersistedContainer;
use ratatui::{
    layout::Rect,
    text::{Line, Masked, Text},
    widgets::Paragraph,
    Frame,
};
use slumber_config::Action;
use std::time::Duration;

const DEBOUNCE: Duration = Duration::from_millis(500);

/// Single line text submission component
#[derive(derive_more::Debug, Default)]
pub struct TextBox {
    // Parameters
    sensitive: bool,
    /// Text to show when text content is empty
    placeholder_text: String,
    /// Text to show when text content is empty and text box is in focus. If
    /// `None`, the default placeholder will be shown instead.
    placeholder_focused: Option<String>,
    /// Predicate function to apply visual validation effect
    #[debug(skip)]
    validator: Option<Validator>,

    // State
    state: TextState,
    on_change_debounce: Option<Debounce>,

    // Callbacks
    /// Called when user clicks to start editing
    #[debug(skip)]
    on_click: Option<Callback>,
    /// Called when user changes the text content. Can optionally be debounced
    #[debug(skip)]
    on_change: Option<Callback>,
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

    /// Set placeholder text to show only while the text box is focused. If not
    /// set, this will fallback to the general placeholder text.
    pub fn placeholder_focused(
        mut self,
        placeholder: impl Into<String>,
    ) -> Self {
        self.placeholder_focused = Some(placeholder.into());
        self
    }

    /// Set validation function. If input is invalid, the `on_change` and
    /// `on_submit` callbacks will be blocked, meaning the user must fix the
    /// error or cancel.
    pub fn validator(
        mut self,
        validator: impl 'static + Fn(&str) -> bool,
    ) -> Self {
        self.validator = Some(Box::new(validator));
        self
    }

    /// Set the callback to be called when the user clicks the textbox
    pub fn on_click(mut self, f: impl 'static + Fn()) -> Self {
        self.on_click = Some(Box::new(f));
        self
    }

    /// Set the callback to be called when the user changes the text value.
    /// Callback can optionally be debounced, so it isn't called repeatedly
    /// while the user is typing
    pub fn on_change(mut self, f: impl 'static + Fn(), debounce: bool) -> Self {
        self.on_change = Some(Box::new(f));
        if debounce {
            self.on_change_debounce = Some(Debounce::new(DEBOUNCE));
        }
        self
    }

    /// Set the callback to be called when the user hits escape
    pub fn on_cancel(mut self, f: impl 'static + Fn()) -> Self {
        self.on_cancel = Some(Box::new(f));
        self
    }

    /// Set the callback to be called when the user hits enter
    pub fn on_submit(mut self, f: impl 'static + Fn()) -> Self {
        self.on_submit = Some(Box::new(f));
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

    /// Handle a key input event, to modify text state. Return `true` if the
    /// event was handled, `false` if it should be propagated
    fn handle_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> bool {
        let text_changed = match code {
            // Don't handle keystrokes if the user is holding a modifier
            KeyCode::Char(c)
                if (modifiers - KeyModifiers::SHIFT).is_empty() =>
            {
                self.state.insert(c);
                true
            }
            KeyCode::Backspace => self.state.delete_left(),
            KeyCode::Delete => self.state.delete_right(),
            KeyCode::Left => {
                if modifiers == KeyModifiers::CONTROL {
                    self.state.home();
                } else {
                    self.state.left();
                }
                false
            }
            KeyCode::Right => {
                if modifiers == KeyModifiers::CONTROL {
                    self.state.end();
                } else {
                    self.state.right();
                }
                false
            }
            KeyCode::Home => {
                self.state.home();
                false
            }
            KeyCode::End => {
                self.state.end();
                false
            }
            _ => return false, // Event should be propagated
        };
        // If text _content_ changed, trigger the auto-submit debounce
        if text_changed {
            self.change();
        }
        true // We DID handle this event
    }

    /// Call parent's on_change callback. Should be called whenever text
    /// _content_ is changed
    fn change(&mut self) {
        let is_valid = self.is_valid();
        if let Some(debounce) = &self.on_change_debounce {
            if self.is_valid() {
                let messages_tx = ViewContext::messages_tx();
                // WARNING: There is a bug here. If there are multiple text
                // boxes active on the screen, there's no guarantee this will go
                // to the right one. Fortunately that UX doesn't really make
                // sense. This can be truly fixed with unique target IDs for
                // local events, but that's a much bigger refactor. For now it
                // should be fine.
                debounce.start(move || {
                    messages_tx.send(Message::Local(Box::new(ChangeEvent)))
                });
            } else {
                debounce.cancel();
            }
        } else if is_valid {
            call(&self.on_change);
        }
    }

    /// Call parent's submission callback
    fn submit(&mut self) {
        if self.is_valid() {
            call(&self.on_submit);
        }
    }
}

impl EventHandler for TextBox {
    fn update(&mut self, _: &mut UpdateContext, event: Event) -> Update {
        match event {
            Event::Local(local)
                if local.downcast_ref::<ChangeEvent>().is_some() =>
            {
                call(&self.on_change)
            }
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
            } => {
                if !self.handle_key(key_event.code, key_event.modifiers) {
                    // Propagate any keystrokes we don't handle (e.g. f keys)
                    return Update::Propagate(event);
                }
            }
            _ => return Update::Propagate(event),
        }
        Update::Consumed
    }
}

impl Draw for TextBox {
    fn draw(&self, frame: &mut Frame, _: (), metadata: DrawMetadata) {
        let styles = &TuiContext::get().styles;

        let text: Text = if self.state.text.is_empty() {
            // Users can optionally set a different placeholder for when focused
            let placeholder = if metadata.has_focus() {
                self.placeholder_focused
                    .as_deref()
                    .unwrap_or(&self.placeholder_text)
            } else {
                &self.placeholder_text
            };
            Line::from(placeholder)
                .style(styles.text_box.placeholder)
                .into()
        } else if self.sensitive {
            // Hide top secret data
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

    /// Delete character immediately left of the cursor. Return `true` if text
    /// was modified
    fn delete_left(&mut self) -> bool {
        if !self.is_at_home() {
            self.left();
            self.text.remove(self.cursor);
            true
        } else {
            false
        }
    }

    /// Delete character immediately rightof the cursor. Return `true` if text
    /// was modified
    fn delete_right(&mut self) -> bool {
        if !self.is_at_end() {
            self.text.remove(self.cursor);
            true
        } else {
            false
        }
    }

    /// Get the **character** offset of the cursor into the text
    fn cursor_offset(&self) -> usize {
        self.text[..self.cursor].chars().count()
    }
}

impl PersistedContainer for TextBox {
    type Value = String;

    fn get_to_persist(&self) -> Self::Value {
        self.state.text.clone()
    }

    fn restore_persisted(&mut self, value: Self::Value) {
        self.set_text(value);
    }
}

/// Local event for triggering debounced on_change calls
#[derive(Debug)]
struct ChangeEvent;

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
        test_util::{harness, terminal, TestHarness, TestTerminal},
        view::test_util::TestComponent,
    };
    use ratatui::text::Span;
    use rstest::rstest;
    use slumber_core::assert_matches;
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
    fn test_interaction(
        harness: TestHarness,
        #[with(10, 1)] terminal: TestTerminal,
    ) {
        let click_count = Counter::default();
        let change_count = Counter::default();
        let submit_count = Counter::default();
        let cancel_count = Counter::default();
        let mut component = TestComponent::new(
            &harness,
            &terminal,
            TextBox::default()
                .on_click(click_count.callback())
                .on_change(change_count.callback(), false)
                .on_submit(submit_count.callback())
                .on_cancel(cancel_count.callback()),
            (),
        );

        // Assert initial state/view
        assert_state(&component.data().state, "", 0);
        terminal.assert_buffer_lines([vec![cursor(" "), text("         ")]]);

        // Type some text
        component.send_text("hello!").assert_empty();
        assert_state(&component.data().state, "hello!", 6);
        terminal.assert_buffer_lines([vec![
            text("hello!"),
            cursor(" "),
            text("   "),
        ]]);

        // Sending with a modifier applied should do nothing, unless it's shift
        component
            .send_key_modifiers(KeyCode::Char('W'), KeyModifiers::SHIFT)
            .assert_empty();
        assert_state(&component.data().state, "hello!W", 7);
        assert_matches!(
            component
                .send_key_modifiers(
                    KeyCode::Char('W'), /* this is what crossterm actually
                                         * sends */
                    KeyModifiers::CONTROL | KeyModifiers::SHIFT,
                )
                .events(),
            &[Event::Input { .. }]
        );
        assert_state(&component.data().state, "hello!W", 7);

        // Test callbacks
        component.click(0, 0).assert_empty();
        assert_eq!(click_count, 1);

        assert_eq!(change_count, 7);

        component.send_key(KeyCode::Enter).assert_empty();
        assert_eq!(submit_count, 1);

        component.send_key(KeyCode::Esc).assert_empty();
        assert_eq!(cancel_count, 1);
    }

    /// Test on_change debouncing
    #[rstest]
    #[tokio::test]
    async fn test_debounce(
        mut harness: TestHarness,
        #[with(10, 1)] terminal: TestTerminal,
    ) {
        let change_count = Counter::default();
        let mut component = TestComponent::new(
            &harness,
            &terminal,
            TextBox::default().on_change(change_count.callback(), true),
            (),
        );

        // Type some text
        component.send_text("hello!").assert_empty();
        // on_change isn't called immediately
        assert_eq!(change_count, 0);

        // It gets called after waiting
        let event = assert_matches!(
            harness.pop_message_wait().await,
            Message::Local(event) => event,
        );
        // We have to feed the event from the message channel to the component
        // manually. This is normally done by the main loop
        component.update_draw(Event::Local(event)).assert_empty();
        assert_eq!(change_count, 1);
    }

    /// Test text navigation and deleting. [TextState] has its own tests so
    /// we're mostly just testing that keys are mapped correctly
    #[rstest]
    fn test_navigation(
        harness: TestHarness,
        #[with(10, 1)] terminal: TestTerminal,
    ) {
        let mut component =
            TestComponent::new(&harness, &terminal, TextBox::default(), ());

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
    fn test_sensitive(
        harness: TestHarness,
        #[with(6, 1)] terminal: TestTerminal,
    ) {
        let mut component = TestComponent::new(
            &harness,
            &terminal,
            TextBox::default().sensitive(true),
            (),
        );

        component.send_text("hello").assert_empty();

        assert_state(&component.data().state, "hello", 5);
        terminal.assert_buffer_lines([vec![text("•••••"), cursor(" ")]]);
    }

    #[rstest]
    fn test_placeholder(
        harness: TestHarness,
        #[with(6, 1)] terminal: TestTerminal,
    ) {
        let component = TestComponent::new(
            &harness,
            &terminal,
            TextBox::default().placeholder("hello"),
            (),
        );

        assert_state(&component.data().state, "", 0);
        let styles = &TuiContext::get().styles.text_box;
        terminal.assert_buffer_lines([vec![
            cursor("h"),
            Span::styled("ello", styles.text.patch(styles.placeholder)),
            text(" "),
        ]]);
    }

    #[rstest]
    fn test_placeholder_focused(
        harness: TestHarness,
        #[with(9, 1)] terminal: TestTerminal,
    ) {
        let mut component = TestComponent::new(
            &harness,
            &terminal,
            TextBox::default()
                .placeholder("unfocused")
                .placeholder_focused("focused"),
            (),
        );
        let styles = &TuiContext::get().styles.text_box;

        // Focused
        assert_state(&component.data().state, "", 0);
        terminal.assert_buffer_lines([vec![
            cursor("f"),
            Span::styled("ocused", styles.text.patch(styles.placeholder)),
            text("  "),
        ]]);

        // Unfocused
        component.unfocus();
        component.drain_draw().assert_empty();
        terminal.assert_buffer_lines([vec![Span::styled(
            "unfocused",
            styles.text.patch(styles.placeholder),
        )]]);
    }

    #[rstest]
    fn test_validator(
        harness: TestHarness,
        #[with(6, 1)] terminal: TestTerminal,
    ) {
        let change_count = Counter::default();
        let submit_count = Counter::default();
        let mut component = TestComponent::new(
            &harness,
            &terminal,
            TextBox::default()
                .validator(|text| text.len() <= 2)
                .on_change(change_count.callback(), false)
                .on_submit(submit_count.callback()),
            (),
        );

        // Valid text, everything is normal
        component.send_text("he").assert_empty();
        terminal.assert_buffer_lines([vec![
            text("he"),
            cursor(" "),
            text("   "),
        ]]);
        assert_eq!(change_count, 2);
        component.send_key(KeyCode::Enter).assert_empty();
        assert_eq!(submit_count, 1);

        // Invalid text, styling changes
        component.send_text("llo").assert_empty();
        terminal.assert_buffer_lines([vec![
            Span::styled("hello", TuiContext::get().styles.text_box.invalid),
            cursor(" "),
        ]]);
        // Callbacks are disabled
        assert_eq!(change_count, 2);
        component.send_key(KeyCode::Enter).assert_empty();
        assert_eq!(submit_count, 1);
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
