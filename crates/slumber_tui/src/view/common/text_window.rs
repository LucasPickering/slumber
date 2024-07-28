use crate::{
    context::TuiContext,
    view::{
        common::scrollbar::Scrollbar,
        draw::{Draw, DrawMetadata},
        event::{Event, EventHandler, Update},
        util::highlight,
    },
};
use ratatui::{
    layout::Layout,
    prelude::{Alignment, Constraint},
    text::{Line, Text},
    widgets::{Paragraph, ScrollbarOrientation},
    Frame,
};
use slumber_config::Action;
use slumber_core::http::content_type::ContentType;
use std::{cell::Cell, cmp};

/// A scrollable (but not editable) block of text. Internal state will be
/// updated on each render, to adjust to the text's width/height.
#[derive(derive_more::Debug, Default)]
pub struct TextWindow {
    offset_x: u16,
    offset_y: u16,
    text_width: Cell<u16>,
    text_height: Cell<u16>,
    window_width: Cell<u16>,
    window_height: Cell<u16>,
}

pub struct TextWindowProps<'a> {
    /// Text to render
    pub text: Text<'a>,
    /// Language of the content; pass to enable syntax highlighting
    pub content_type: Option<ContentType>,
    /// Is there a search box below the content? This tells us if we need to
    /// offset the horizontal scroll box an extra row.
    pub has_search_box: bool,
}

impl TextWindow {
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

impl EventHandler for TextWindow {
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
impl<'a> Draw<TextWindowProps<'a>> for TextWindow {
    fn draw(
        &self,
        frame: &mut Frame,
        props: TextWindowProps<'a>,
        metadata: DrawMetadata,
    ) {
        let styles = &TuiContext::get().styles;

        // Apply syntax highlighting
        let text = if let Some(content_type) = props.content_type {
            highlight::highlight(content_type, props.text)
        } else {
            props.text
        };

        let text = Paragraph::new(text);
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
