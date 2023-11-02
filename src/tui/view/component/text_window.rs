use crate::tui::{
    input::Action,
    view::{
        component::{Component, Draw, DrawContext, Event, UpdateOutcome},
        util::layout,
    },
};
use derive_more::Display;
use ratatui::{
    prelude::{Alignment, Constraint, Direction, Rect},
    text::{Line, Text},
    widgets::Paragraph,
};
use std::cmp;

/// A view of text that can be scrolled through vertically. This should be used
/// for *immutable* text only.
///
/// TODO try TextArea instead?
///
/// Some day hopefully we can get rid of this in favor of a widget from ratatui
/// https://github.com/ratatui-org/ratatui/issues/174
#[derive(Debug, Default, Display)]
#[display(fmt = "TextWindow")]
pub struct TextWindow {
    offset_y: u16,
}

impl TextWindow {
    /// Reset scroll state
    pub fn reset(&mut self) {
        self.offset_y = 0;
    }

    fn up(&mut self) {
        self.offset_y = self.offset_y.saturating_sub(1);
    }

    fn down(&mut self) {
        self.offset_y += 1;
    }
}

pub struct TextWindowProps<'a> {
    pub text: Text<'a>,
}

impl Component for TextWindow {
    fn update(
        &mut self,
        _context: &mut super::UpdateContext,
        event: Event,
    ) -> UpdateOutcome {
        match event {
            Event::Input {
                action: Some(action),
                ..
            } => match action {
                Action::Up => {
                    self.up();
                    UpdateOutcome::Consumed
                }
                Action::Down => {
                    self.down();
                    UpdateOutcome::Consumed
                }
                _ => UpdateOutcome::Propagate(event),
            },
            _ => UpdateOutcome::Propagate(event),
        }
    }
}

impl<'a> Draw<TextWindowProps<'a>> for TextWindow {
    fn draw(
        &self,
        context: &mut DrawContext,
        props: TextWindowProps<'a>,
        chunk: Rect,
    ) {
        let num_lines = props.text.lines.len() as u16;

        let [gutter_chunk, _, text_chunk] = layout(
            chunk,
            Direction::Horizontal,
            [
                // Size gutter based on max line number width
                Constraint::Length(
                    (num_lines as f32).log10().floor() as u16 + 1,
                ),
                Constraint::Length(1), // Spacer gap
                Constraint::Min(0),
            ],
        );

        // Add line numbers to the gutter
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

        context.frame.render_widget(
            Paragraph::new(props.text).scroll((self.offset_y, 0)),
            text_chunk,
        );
    }
}
