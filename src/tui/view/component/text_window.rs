use crate::tui::{
    input::Action,
    view::{
        component::{Component, Draw, DrawContext, Event, Update},
        util::{layout, ToTui},
    },
};
use derive_more::Display;
use ratatui::{
    prelude::{Alignment, Constraint, Direction, Rect},
    text::{Line, Text},
    widgets::Paragraph,
};
use std::{cell::Cell, cmp, fmt::Debug};

/// A scrollable (but not editable) block of text. Text is not externally
/// mutable. If you need to update the text, store this in a `StateCell` and
/// reconstruct the entire component.
///
/// The generic parameter allows for any type that can be converted to ratatui's
/// `Text`, e.g. `String` or `TemplatePreview`.
#[derive(Debug, Display)]
#[display(fmt = "TextWindow")]
pub struct TextWindow<T> {
    text: T,
    offset_y: u16,
    text_height: Cell<u16>,
    window_height: Cell<u16>,
}

impl<T> TextWindow<T> {
    pub fn new(text: T) -> Self {
        Self {
            text,
            offset_y: 0,
            text_height: Cell::default(),
            window_height: Cell::default(),
        }
    }

    fn scroll_up(&mut self, lines: u16) {
        self.offset_y = self.offset_y.saturating_sub(lines);
    }

    fn scroll_down(&mut self, lines: u16) {
        self.offset_y = cmp::min(
            self.offset_y + lines,
            // Don't scroll past the bottom of the text
            self.text_height
                .get()
                .saturating_sub(self.window_height.get()),
        );
    }
}

impl<T: Debug> Component for TextWindow<T> {
    fn update(
        &mut self,
        _context: &mut super::UpdateContext,
        event: Event,
    ) -> Update {
        match event {
            Event::Input {
                action: Some(Action::Up),
                ..
            } => {
                self.scroll_up(1);
                Update::Consumed
            }
            Event::Input {
                action: Some(Action::Down),
                ..
            } => {
                self.scroll_down(1);
                Update::Consumed
            }
            _ => Update::Propagate(event),
        }
    }
}

impl<'a, T: 'a + ToTui<Output<'a> = Text<'a>>> Draw for &'a TextWindow<T> {
    fn draw(&self, context: &mut DrawContext, _: (), chunk: Rect) {
        let text = self.text.to_tui(context);
        // TODO how do we handle text longer than 65k lines?
        let text_height = text.lines.len() as u16;
        self.text_height.set(text_height);
        self.window_height.set(chunk.height);

        let [gutter_chunk, _, text_chunk] = layout(
            chunk,
            Direction::Horizontal,
            [
                // Size gutter based on width of max line number
                Constraint::Length(
                    (text_height as f32).log10().floor() as u16 + 1,
                ),
                Constraint::Length(1), // Spacer
                Constraint::Min(0),
            ],
        );

        // Draw line numbers in the gutter
        let first_line = self.offset_y + 1;
        let last_line = cmp::min(first_line + chunk.height, text_height);
        context.frame.render_widget(
            Paragraph::new(
                (first_line..=last_line)
                    .map(|n| n.to_string().into())
                    .collect::<Vec<Line>>(),
            )
            .alignment(Alignment::Right),
            gutter_chunk,
        );

        // Darw the text content
        context.frame.render_widget(
            Paragraph::new(self.text.to_tui(context))
                .scroll((self.offset_y, 0)),
            text_chunk,
        );
    }
}
