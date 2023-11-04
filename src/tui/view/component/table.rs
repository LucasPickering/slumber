use crate::tui::view::component::{Draw, DrawContext};
use derive_more::Display;
use ratatui::{
    prelude::{Constraint, Rect},
    widgets::Row,
};
use std::fmt::Display;

/// 2-column tabular data display
#[derive(Debug, Display)]
pub struct Table;

pub struct TableProps<'a, T> {
    pub key_label: &'a str,
    pub value_label: &'a str,
    pub data: T,
}

/// Any (key, value) iterator can be drawn as a table, as long as the key and
/// value implement `Display`
impl<'a, K, V, Iter, Data> Draw<TableProps<'a, Data>> for Table
where
    K: Display,
    V: Display,
    Iter: Iterator<Item = (K, V)>,
    Data: IntoIterator<Item = (K, V), IntoIter = Iter>,
{
    fn draw(
        &self,
        context: &mut DrawContext,
        props: TableProps<'a, Data>,
        chunk: Rect,
    ) {
        let rows = props.data.into_iter().enumerate().map(|(i, (k, v))| {
            // Alternate row style for readability
            let style = if i % 2 == 0 {
                context.theme.table_text_style
            } else {
                context.theme.table_alt_text_style
            };
            Row::new(vec![k.to_string(), v.to_string()]).style(style)
        });
        let table = ratatui::widgets::Table::new(rows)
            .header(
                Row::new(vec![props.key_label, props.value_label])
                    .style(context.theme.table_header_style),
            )
            .widths(&[Constraint::Percentage(50), Constraint::Percentage(50)]);
        context.frame.render_widget(table, chunk)
    }
}
