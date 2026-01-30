//! The [`ClearFill`] widget allows you to clear a certain area with a
//! background color to allow overdrawing (e.g. for popups).
use crate::view::context::ViewContext;
use ratatui::{
    buffer::{Buffer, Cell},
    layout::Rect,
    style::Style,
    widgets::{Clear, Widget},
};

/// A widget to clear/reset a certain area with a background color to allow
/// overdrawing (e.g. for popups).
#[derive(Debug, Default, Clone, Eq, PartialEq, Hash)]
pub struct ClearFill {
    style: Option<Style>,
}

impl Widget for ClearFill {
    fn render(self, area: Rect, buf: &mut Buffer) {
        Widget::render(&self, area, buf);
    }
}

impl Widget for &ClearFill {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if let Some(cell) = self.background_cell() {
            for x in area.left()..area.right() {
                for y in area.top()..area.bottom() {
                    buf[(x, y)] = cell.clone();
                }
            }
        } else {
            // No background color is set, we can defer to the `Clear` widget
            Clear.render(area, buf);
        }
    }
}

impl ClearFill {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_style(style: Style) -> Self {
        Self { style: Some(style) }
    }

    pub fn background_cell(&self) -> Option<Cell> {
        let style = self.style.unwrap_or(ViewContext::styles().root.background);
        style.bg.map(|bg_color| {
            let mut cell = Cell::default();
            cell.set_bg(bg_color);
            cell
        })
    }
}
