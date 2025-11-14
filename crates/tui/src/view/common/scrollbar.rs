use ratatui::{
    buffer::Buffer,
    layout::{Offset, Rect},
    widgets::{ScrollbarOrientation, ScrollbarState, StatefulWidget, Widget},
};

/// A wrapper around Ratatui's scrollbar to make it more ergonomic. This has a
/// few main purposes:
/// - Standardize styling
/// - Handle margin offsets
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
    /// Where is the scrollbar placed?
    pub orientation: ScrollbarOrientation,
    /// Invert the display of the scrollbar, so that the first item is at the
    /// bottom (for vertical) or right (for horizontal)
    pub invert: bool,
}

impl Default for Scrollbar {
    fn default() -> Self {
        Self {
            content_length: 0,
            offset: 0,
            margin: 1,
            orientation: ScrollbarOrientation::VerticalRight,
            invert: false,
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
        // there are. 1 for the current viewport + the number of items outside
        // the viewport (on either side).
        //
        // If the entire content fits in the viewport, use 0 to hide the scroll
        let content_length = if self.content_length <= size {
            0
        } else {
            self.content_length.saturating_sub(size) + 1
        };
        // position is the index of the first *visible* element
        let position = if self.invert {
            if self.offset < content_length {
                content_length - self.offset - 1
            } else {
                0
            }
        } else {
            self.offset
        };
        ScrollbarState::new(content_length).position(position)
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
    #[case::empty(0, 0, false, ScrollbarState::new(0).position(0))]
    #[case::extra_space(9, 0, false, ScrollbarState::new(0).position(0))]
    #[case::perfect_fit_first(10, 0, false, ScrollbarState::new(0).position(0))]
    #[case::perfect_fit_last(10, 9, false, ScrollbarState::new(0).position(0))]
    // Overflow without offset
    #[case::overflow(11, 0, false, ScrollbarState::new(2).position(0))]
    // We scrolled down, but not far enough to move the scrollbar
    #[case::overflow_offset(11, 3, false, ScrollbarState::new(2).position(0))]
    // Scroll down far enough to move the scrollbar to an intermediate position
    #[case::overflow_scrolled(12, 10, false, ScrollbarState::new(3).position(1))]
    // Last item is selected, so items 10-19 are visible
    #[case::overflow_scrolled_bottom(20, 19, false, ScrollbarState::new(11).position(10))]
    // Inversion!!
    #[case::invert_empty(0, 0, true, ScrollbarState::new(0).position(0))]
    #[case::invert_extra_space(9, 0, true, ScrollbarState::new(0).position(0))]
    #[case::invert_perfect_fit_first(10, 0, true, ScrollbarState::new(0).position(0))]
    #[case::invert_perfect_fit_last(10, 9, true, ScrollbarState::new(0).position(0))]
    // With invert=true, selected=0 is the bottom of the list. Items 0-9 are
    // visible, which is 1-10 when flipped
    #[case::invert_overflow(11, 0, true, ScrollbarState::new(2).position(1))]
    // We scrolled up, but not far enough to move the scrollbar
    #[case::invert_overflow_offset(11, 3, true, ScrollbarState::new(2).position(1))]
    // Scroll up far enough to move the scrollbar to an intermediate position
    #[case::invert_overflow_scrolled(12, 10, true, ScrollbarState::new(3).position(1))]
    // Last (top) item is selected, so items 10-19 are visible
    #[case::invert_overflow_scrolled_top(20, 19, true, ScrollbarState::new(11).position(0))]
    fn test_state(
        #[case] content_length: usize,
        #[case] selected: usize,
        #[case] invert: bool,
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
            invert,
        };
        assert_eq!(scrollbar.state(area), expected);
    }
}
