use crate::tui::{
    context::TuiContext,
    input::Action,
    view::{
        common::scrollbar::Scrollbar,
        draw::{Draw, DrawMetadata, Generate},
        event::{Event, EventHandler, Update},
    },
};
use ratatui::{
    layout::Layout,
    prelude::{Alignment, Constraint},
    text::{Line, Text},
    widgets::{Paragraph, ScrollbarOrientation},
    Frame,
};
use std::{cell::Cell, cmp, fmt::Debug};

/// A scrollable (but not editable) block of text. Text is not externally
/// mutable. If you need to update the text, store this in a `StateCell` and
/// reconstruct the entire component.
///
/// The generic parameter allows for any type that can be converted to ratatui's
/// `Text`, e.g. `String` or `TemplatePreview`.
#[derive(Debug, Default)]
pub struct TextWindow<T> {
    text: T,
    offset_x: u16,
    offset_y: u16,
    text_width: Cell<u16>,
    text_height: Cell<u16>,
    window_width: Cell<u16>,
    window_height: Cell<u16>,
}

#[derive(Default)]
pub struct TextWindowProps {
    /// Is there a search box below the content? This tells us if we need to
    /// offset the horizontal scroll box an extra row.
    pub has_search_box: bool,
}

impl<T> TextWindow<T> {
    pub fn new(text: T) -> Self {
        Self {
            text,
            offset_x: 0,
            offset_y: 0,
            text_width: Cell::default(),
            text_height: Cell::default(),
            window_width: Cell::default(),
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

    /// Get the final column that we can't scroll (horizontally) past. This will
    /// be the left edge of the rightmost "page" of text
    fn max_scroll_column(&self) -> u16 {
        self.text_width
            .get()
            .saturating_sub(self.window_width.get())
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

    fn scroll_left(&mut self, columns: u16) {
        self.offset_x = self.offset_x.saturating_sub(columns);
    }

    fn scroll_right(&mut self, columns: u16) {
        self.offset_x =
            cmp::min(self.offset_x + columns, self.max_scroll_column());
    }
}

impl<T: Debug> EventHandler for TextWindow<T> {
    fn update(&mut self, event: Event) -> Update {
        let Some(action) = event.action() else {
            return Update::Propagate(event);
        };
        match action {
            Action::Up | Action::ScrollUp => self.scroll_up(1),
            Action::Down | Action::ScrollDown => self.scroll_down(1),
            Action::ScrollLeft => self.scroll_left(1),
            Action::ScrollRight => self.scroll_right(1),
            Action::PageUp => self.scroll_up(self.window_height.get()),
            Action::PageDown => self.scroll_down(self.window_height.get()),
            Action::Home => self.scroll_to(0),
            Action::End => self.scroll_to(u16::MAX),
            _ => return Update::Propagate(event),
        }
        Update::Consumed
    }
}

/// `T` has to be convertible to text to be drawn
impl<T> Draw<TextWindowProps> for TextWindow<T>
where
    T: 'static,
    for<'a> &'a T: Generate<Output<'a> = Text<'a>>,
{
    fn draw(
        &self,
        frame: &mut Frame,
        props: TextWindowProps,
        metadata: DrawMetadata,
    ) {
        let styles = &TuiContext::get().styles;
        let text = Paragraph::new(self.text.generate());
        // Assume no line wrapping when calculating line count
        let text_height = text.line_count(u16::MAX) as u16;

        let [gutter_area, _, text_area] = Layout::horizontal([
            // Size gutter based on width of max line number
            Constraint::Length((text_height as f32).log10().floor() as u16 + 1),
            Constraint::Length(1), // Spacer
            Constraint::Min(0),
        ])
        .areas(metadata.area());

        // Store text and window sizes for calculations in the update code
        self.text_width.set(text.line_width() as u16);
        self.text_height.set(text_height);
        self.window_width.set(text_area.width);
        self.window_height.set(text_area.height);

        // Draw line numbers in the gutter
        let first_line = self.offset_y + 1;
        let last_line = cmp::min(first_line + text_area.height, text_height);
        frame.render_widget(
            Paragraph::new(
                (first_line..=last_line)
                    .map(|n| n.to_string().into())
                    .collect::<Vec<Line>>(),
            )
            .alignment(Alignment::Right)
            .style(styles.text_window.gutter),
            gutter_area,
        );

        // Draw the text content
        frame.render_widget(
            text.scroll((self.offset_y, self.offset_x)),
            text_area,
        );

        // Scrollbars
        frame.render_widget(
            Scrollbar {
                content_length: self.text_height.get() as usize,
                offset: self.offset_y as usize,
                ..Default::default()
            },
            text_area,
        );
        frame.render_widget(
            Scrollbar {
                content_length: self.text_width.get() as usize,
                offset: self.offset_x as usize,
                orientation: ScrollbarOrientation::HorizontalBottom,
                margin: if props.has_search_box { 2 } else { 1 },
            },
            text_area,
        );
    }
}
