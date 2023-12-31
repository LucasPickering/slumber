use crate::tui::{
    context::TuiContext,
    input::Action,
    view::{
        draw::{Draw, Generate},
        event::{Event, EventHandler, Update, UpdateContext},
        util::layout,
    },
};
use ratatui::{
    prelude::{Alignment, Constraint, Direction, Rect},
    text::{Line, Text},
    widgets::Paragraph,
    Frame,
};
use std::{cell::Cell, cmp, fmt::Debug};

/// A scrollable (but not editable) block of text. Text is not externally
/// mutable. If you need to update the text, store this in a `StateCell` and
/// reconstruct the entire component.
///
/// The generic parameter allows for any type that can be converted to ratatui's
/// `Text`, e.g. `String` or `TemplatePreview`.
#[derive(derive_more::Debug)]
pub struct TextWindow<T> {
    #[debug(skip)]
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

    pub fn text(&self) -> &T {
        &self.text
    }

    /// Get the final line that we can't scroll past. This will be the first
    /// line of the last page of text
    fn max_scroll_line(&self) -> u16 {
        self.text_height
            .get()
            .saturating_sub(self.window_height.get())
    }

    fn scroll_up(&mut self, lines: u16) {
        self.offset_y = self.offset_y.saturating_sub(lines);
    }

    fn scroll_down(&mut self, lines: u16) {
        self.offset_y = cmp::min(self.offset_y + lines, self.max_scroll_line());
    }

    /// Scroll to a specific line number. The target line will end up as close
    /// to the top of the page as possible
    fn scroll_to(&mut self, line: u16) {
        self.offset_y = cmp::min(line, self.max_scroll_line());
    }
}

impl<T: Debug> EventHandler for TextWindow<T> {
    fn update(&mut self, _context: &mut UpdateContext, event: Event) -> Update {
        match event {
            Event::Input {
                action: Some(action),
                ..
            } => match action {
                Action::Up | Action::ScrollUp => self.scroll_up(1),
                Action::Down | Action::ScrollDown => self.scroll_down(1),
                Action::PageUp => self.scroll_up(self.window_height.get()),
                Action::PageDown => self.scroll_down(self.window_height.get()),
                Action::Home => self.scroll_to(0),
                Action::End => self.scroll_to(u16::MAX),
                _ => return Update::Propagate(event),
            },
            _ => return Update::Propagate(event),
        }
        Update::Consumed
    }
}

impl<'a, T> Draw for &'a TextWindow<T>
where
    &'a T: 'a + Generate<Output<'a> = Text<'a>>,
{
    fn draw(&self, frame: &mut Frame, _: (), area: Rect) {
        let theme = &TuiContext::get().theme;
        let text = self.text.generate();
        let text_height = text.lines.len() as u16;
        self.text_height.set(text_height);
        self.window_height.set(area.height);

        let [gutter_area, _, text_area] = layout(
            area,
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
        let last_line = cmp::min(first_line + area.height, text_height);
        frame.render_widget(
            Paragraph::new(
                (first_line..=last_line)
                    .map(|n| n.to_string().into())
                    .collect::<Vec<Line>>(),
            )
            .alignment(Alignment::Right)
            .style(theme.line_number_style),
            gutter_area,
        );

        // Darw the text content
        frame.render_widget(
            Paragraph::new(self.text.generate()).scroll((self.offset_y, 0)),
            text_area,
        );
    }
}
