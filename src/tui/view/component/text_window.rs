use crate::tui::{
    input::Action,
    view::{
        component::{Component, Draw, DrawContext, Event, Update},
        theme::Theme,
    },
};
use derive_more::Display;
use ratatui::{prelude::Rect, style::Style};
use std::{cell::RefCell, fmt::Debug, ops::Deref};
use tui_textarea::TextArea;

/// A scrollable (but not editable) block of text. The `Key` parameter is used
/// to tell the text window when to reset its internal state. The type should be
/// cheap to compare (e.g. a `Uuid` or short string), and the value is passed to
/// the `draw` function as a prop. Whenever the value changes, the text buffer
/// will be reset to the content of the `text` prop on that draw. As such, the
/// key and text should be in sync: when one changes, the other does too.
#[derive(Debug, Display)]
#[display(fmt = "TextWindow")]
pub struct TextWindow<Key> {
    /// State is stored in a refcell so it can be mutated during the draw. It
    /// can be very hard to drill down the text content in the update phase, so
    /// this makes it transparent to the caller.
    ///
    /// `RefCell` is safe here because its accesses are never held across
    /// phases, and all view code is synchronous.
    state: RefCell<Option<State<Key>>>,
}

pub struct TextWindowProps<'a, Key> {
    pub key: &'a Key,
    pub text: &'a str,
}

#[derive(Debug)]
struct State<Key> {
    key: Key,
    text_area: TextArea<'static>,
}

impl<Key: Debug> Component for TextWindow<Key> {
    fn update(
        &mut self,
        _context: &mut super::UpdateContext,
        event: Event,
    ) -> Update {
        // Don't handle any events if state isn't initialized yet
        if let Some(state) = self.state.get_mut() {
            match event {
                Event::Input {
                    action: Some(Action::Up),
                    ..
                } => {
                    state.text_area.scroll((-1, 0));
                    Update::Consumed
                }
                Event::Input {
                    action: Some(Action::Down),
                    ..
                } => {
                    state.text_area.scroll((1, 0));
                    Update::Consumed
                }
                _ => Update::Propagate(event),
            }
        } else {
            Update::Propagate(event)
        }
    }
}

impl<'a, Key: Clone + Debug + PartialEq> Draw<TextWindowProps<'a, Key>>
    for TextWindow<Key>
{
    fn draw(
        &self,
        context: &mut DrawContext,
        props: TextWindowProps<'a, Key>,
        chunk: Rect,
    ) {
        // This uses a reactive pattern to initialize the text area. The key
        // should change whenever the text does, and that signals to rebuild the
        // text area.

        // Check if the data is either uninitialized or outdated
        {
            let mut state = self.state.borrow_mut();
            match state.deref() {
                Some(state) if &state.key == props.key => {}
                _ => {
                    // (Re)create the state
                    *state = Some(State {
                        key: props.key.clone(),
                        text_area: init_text_area(context.theme, props.text),
                    });
                }
            }
        }

        // Unwrap is safe because we know we just initialized state above
        let state = self.state.borrow();
        let text_area = &state.as_ref().unwrap().text_area;
        context.frame.render_widget(text_area.widget(), chunk);
    }
}

/// Derive impl applies unnecessary bound on the generic parameter
impl<Key> Default for TextWindow<Key> {
    fn default() -> Self {
        Self {
            state: RefCell::new(None),
        }
    }
}

fn init_text_area(theme: &Theme, text: &str) -> TextArea<'static> {
    let mut text_area: TextArea = text.lines().map(str::to_owned).collect();
    // Hide cursor/line selection highlights
    text_area.set_cursor_style(Style::default());
    text_area.set_cursor_line_style(Style::default());
    text_area.set_line_number_style(theme.line_number_style);
    text_area
}
