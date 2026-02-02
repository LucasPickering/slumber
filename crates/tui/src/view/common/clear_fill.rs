//! The [`ClearFill`] widget allows you to clear a certain area with a
//! background color to allow overdrawing (e.g. for popups).
use crate::view::context::ViewContext;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    widgets::{Block, Clear, Widget},
};

/// A widget to clear/reset a certain area with a background color to allow
/// overdrawing (e.g. for popups).
#[derive(Debug)]
pub struct ClearFill;

impl Widget for ClearFill {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let style = ViewContext::styles().background;
        Clear.render(area, buf);
        Block::default().style(style).render(area, buf);
    }
}
