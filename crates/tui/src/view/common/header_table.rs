use crate::view::{Generate, common::table::Table};
use itertools::Itertools;
use ratatui::{
    prelude::{Buffer, Rect},
    text::Text,
    widgets::Widget,
};
use reqwest::header::HeaderMap;

/// Render HTTP request/response headers in a table
pub struct HeaderTable<'a> {
    pub headers: &'a HeaderMap,
}

impl Widget for HeaderTable<'_> {
    fn render(self, area: Rect, buf: &mut Buffer)
    where
        Self: Sized,
    {
        Table {
            rows: self
                .headers
                .iter()
                .map(|(k, v)| [Text::from(k.as_str()), v.generate().into()])
                .collect_vec(),
            header: Some(["Header", "Value"]),
            alternate_row_style: true,
            ..Default::default()
        }
        .render(area, buf);
    }
}
