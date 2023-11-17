use crate::tui::view::{draw::Generate, theme::Theme};
use ratatui::{
    prelude::Constraint,
    widgets::{Cell, Row},
};

/// Tabular data display with a static number of columns
#[derive(Debug)]
pub struct Table<'a, const COLS: usize, Rows> {
    pub rows: Rows,
    /// Optional header row. Length should match column length
    pub header: Option<[&'a str; COLS]>,
    /// Use a different styling for alternating rows
    pub alternate_row_style: bool,
    /// Take an array ref (NOT a slice) so we can enforce the length, but the
    /// lifetime can outlive this struct
    pub column_widths: &'a [Constraint; COLS],
}

impl<'a, const COLS: usize, Rows: Default> Default for Table<'a, COLS, Rows> {
    fn default() -> Self {
        Self {
            rows: Default::default(),
            header: None,
            alternate_row_style: true,
            // Evenly spaced by default
            column_widths: &[Constraint::Ratio(1, COLS as u32); COLS],
        }
    }
}

impl<'a, const COLS: usize, Cll, Rows> Generate for Table<'a, COLS, Rows>
where
    Cll: Into<Cell<'a>>,
    Rows: IntoIterator<Item = [Cll; COLS]>,
{
    type Output<'this> = ratatui::widgets::Table<'this>
    where
        Self: 'this;

    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this,
    {
        let theme = Theme::get();
        let rows = self.rows.into_iter().enumerate().map(|(i, row)| {
            // Alternate row style for readability
            let style = if self.alternate_row_style && i % 2 == 1 {
                theme.table_alt_text_style
            } else {
                theme.table_text_style
            };
            Row::new(row).style(style)
        });
        let mut table = ratatui::widgets::Table::new(rows)
            .highlight_style(theme.table_highlight_style)
            .widths(self.column_widths);

        // Add optional header if given
        if let Some(header) = self.header {
            table =
                table.header(Row::new(header).style(theme.table_header_style));
        }

        table
    }
}
