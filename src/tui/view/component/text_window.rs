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
use std::{cmp, fmt::Debug};

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
}

impl<T> TextWindow<T> {
    pub fn new(text: T) -> Self {
        Self { text, offset_y: 0 }
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
                self.offset_y = self.offset_y.saturating_sub(1);
                Update::Consumed
            }
            Event::Input {
                action: Some(Action::Down),
                ..
            } => {
                // TODO upper bound on scroll. It's doable because we have the
                // text, but we need to work through the generics somehow
                self.offset_y += 1;
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
        let num_lines = text.lines.len() as u16;

        let [gutter_chunk, _, text_chunk] = layout(
            chunk,
            Direction::Horizontal,
            [
                // Size gutter based on width of max line number
                Constraint::Length(
                    (num_lines as f32).log10().floor() as u16 + 1,
                ),
                Constraint::Length(1), // Spacer
                Constraint::Min(0),
            ],
        );

        // Draw line numbers in the gutter
        let first_line = self.offset_y + 1;
        let last_line = cmp::min(first_line + chunk.height, num_lines);
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
