use ratatui::{
    buffer::Buffer,
    layout::{Offset, Rect},
    widgets::{ScrollbarOrientation, ScrollbarState, StatefulWidget, Widget},
};

/// A wrapper around Ratatui's scrollbar to make it more ergonomic. This has two
/// main purposes:
/// - Standardize styling
/// - Handle annoying state calculation
#[derive(Clone, Debug)]
pub struct Scrollbar {
    /// Number of rows in your content, e.g. items in a list or lines in a
    /// text file. For horizontal scrolling, this is the number of columns.
    pub content_length: usize,
    /// Visual offset into the content, i.e. the index of the first visible
    /// item
    pub offset: usize,
    /// How far should the scrollbar be offset from its content? Positive to
    /// offset out, negative to offset in. Defaults to 1, because most content
    /// has a border that can contain the scrollbar.
    pub margin: i32,
    /// Which way should the scrollbar face?
    pub orientation: ScrollbarOrientation,
}

impl Default for Scrollbar {
    fn default() -> Self {
        Self {
            content_length: 0,
            offset: 0,
            margin: 1,
            orientation: ScrollbarOrientation::VerticalRight,
        }
    }
}

impl Scrollbar {
    fn state(&self, area: Rect) -> ScrollbarState {
        let size = match &self.orientation {
            ScrollbarOrientation::VerticalRight
            | ScrollbarOrientation::VerticalLeft => area.height,
            ScrollbarOrientation::HorizontalBottom
            | ScrollbarOrientation::HorizontalTop => area.width,
        } as usize;

        // To Ratatui, content_length is how many possible scroll positions
        // there are, which is the number of items beyond the viewport, plus one
        // to capture all items in the viewport. If the entire content fits in
        // the viewport though, we just use 0 to hide the scroll
        let content_length = if self.content_length <= size {
            0
        } else {
            self.content_length.saturating_sub(size) + 1
        };
        ScrollbarState::new(content_length)
            // position is the index of the first *visible* element
            .position(self.offset)
    }
}

impl Widget for Scrollbar {
    fn render(self, area: Rect, buf: &mut Buffer)
    where
        Self: Sized,
    {
        let (begin_symbol, thumb_symbol, end_symbol) = match &self.orientation {
            ScrollbarOrientation::VerticalRight
            | ScrollbarOrientation::VerticalLeft => ("▲", "█", "▼"),
            ScrollbarOrientation::HorizontalBottom
            | ScrollbarOrientation::HorizontalTop => ("◀", "■", "▶"),
        };

        // Apply an offset to put this outside the content
        let offset = match &self.orientation {
            ScrollbarOrientation::VerticalRight => Offset {
                x: self.margin,
                y: 0,
            },
            ScrollbarOrientation::VerticalLeft => Offset {
                x: -self.margin,
                y: 0,
            },
            ScrollbarOrientation::HorizontalBottom => Offset {
                x: 0,
                y: self.margin,
            },
            ScrollbarOrientation::HorizontalTop => Offset {
                x: 0,
                y: -self.margin,
            },
        };

        let scrollbar =
            ratatui::widgets::Scrollbar::new(self.orientation.clone())
                .begin_symbol(Some(begin_symbol))
                .thumb_symbol(thumb_symbol)
                .end_symbol(Some(end_symbol));

        let area = area.offset(offset);
        // Avoid panic if there's nowhere to render the scroll bar. This can
        // occur if the screen gets really small
        if !area.is_empty() {
            StatefulWidget::render(scrollbar, area, buf, &mut self.state(area));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{
        buffer::Buffer,
        widgets::{List, ListState, ScrollbarState},
    };
    use rstest::rstest;

    #[rstest]
    // If len <= height, we should have *no* scrollbar
    #[case::empty(0, 0, ScrollbarState::new(0).position(0))]
    #[case::extra_space(9, 0, ScrollbarState::new(0).position(0))]
    #[case::perfect_fit_first(10, 0, ScrollbarState::new(0).position(0))]
    #[case::perfect_fit_last(10, 0, ScrollbarState::new(0).position(0))]
    // Overflow without offset
    #[case::overflow(11, 0, ScrollbarState::new(2).position(0))]
    // We scrolled down, but not far enough to move the scrollbar
    #[case::overflow_offset(11, 3, ScrollbarState::new(2).position(0))]
    // We scrolled down far enough to move the scrollbar
    #[case::overflow_scrolled(12, 10, ScrollbarState::new(3).position(1))]
    #[case::overflow_scrolled_bottom(20, 19, ScrollbarState::new(11).position(10))]
    fn test_state(
        #[case] content_length: usize,
        #[case] selected: usize,
        #[case] expected: ScrollbarState,
    ) {
        let area = Rect::new(0, 0, 5, 10);

        // Render a list once to get a realistic offset calculation
        let mut buffer = Buffer::empty(area);
        let list: List = (0..content_length).map(|i| i.to_string()).collect();
        let mut state = ListState::default().with_selected(Some(selected));
        StatefulWidget::render(list, area, &mut buffer, &mut state);

        let scrollbar = Scrollbar {
            content_length,
            offset: state.offset(),
            margin: 0,
            orientation: ScrollbarOrientation::VerticalRight,
        };
        assert_eq!(scrollbar.state(area), expected);
    }
}
