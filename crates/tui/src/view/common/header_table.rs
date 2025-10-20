use crate::view::{Generate, common::table::Table};
use itertools::Itertools;
use ratatui::text::Text;
use reqwest::header::HeaderMap;

/// Render HTTP request/response headers in a table
pub struct HeaderTable<'a> {
    pub headers: &'a HeaderMap,
}

impl Generate for HeaderTable<'_> {
    type Output<'this>
        = ratatui::widgets::Table<'this>
    where
        Self: 'this;

    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this,
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
        .generate()
    }
}
