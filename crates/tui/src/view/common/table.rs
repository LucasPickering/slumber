use crate::view::context::ViewContext;
use ratatui::{
    prelude::{Buffer, Constraint, Rect},
    style::Styled,
    widgets::{Block, Cell, Row, StatefulWidget, TableState, Widget},
};

/// Tabular data display with a static number of columns.
///
/// The `R` generic defines the row type, which should be either an array of
/// cell types (e.g. `[Text; 3]`) or [ratatui::widgets::Row]. If using an array,
/// the length should match `COLS`. Allowing `Row` makes it possible to override
/// styling on a row-by-row basis.
///
/// This is a thing wrapper around [ratatui::widgets::Table] that handles a lot
/// of the boilerplate we need for our specific applications.
#[derive(Debug)]
pub struct Table<'a, const COLS: usize, R> {
    pub title: Option<&'a str>,
    pub rows: Vec<R>,
    /// Optional header row. Length should match column length
    pub header: Option<[&'a str; COLS]>,
    /// Use a different styling for alternating rows
    pub alternate_row_style: bool,
    /// Take an array ref (NOT a slice) so we can enforce the length, but the
    /// lifetime can outlive this struct
    pub column_widths: &'a [Constraint; COLS],
}

impl<const COLS: usize, Rows: Default> Default for Table<'_, COLS, Rows> {
    fn default() -> Self {
        Self {
            title: None,
            rows: Default::default(),
            header: None,
            alternate_row_style: false,
            // Evenly spaced by default
            column_widths: &[Constraint::Ratio(1, COLS as u32); COLS],
        }
    }
}

/// Render a table with fixed columns and no selection state
impl<'a, const COLS: usize, Cll> Widget for Table<'a, COLS, [Cll; COLS]>
where
    Cll: Into<Cell<'a>>,
{
    fn render(self, area: Rect, buf: &mut Buffer) {
        StatefulWidget::render(self, area, buf, &mut TableState::default());
    }
}

/// Render a table with fixed columns and selection state
impl<'a, const COLS: usize, Cll> StatefulWidget for Table<'a, COLS, [Cll; COLS]>
where
    Cll: Into<Cell<'a>>,
{
    type State = TableState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        let styles = ViewContext::styles();
        let rows = self.rows.into_iter().enumerate().map(|(i, row)| {
            let row = Row::new(row);
            // Apply theme styles, but let the row's individual styles override
            let base_style = if self.alternate_row_style && i % 2 == 1 {
                styles.table.alt
            } else {
                styles.table.text
            };
            let row_style = Styled::style(&row);
            row.set_style(base_style.patch(row_style))
        });
        let mut table = ratatui::widgets::Table::new(rows, self.column_widths)
            .row_highlight_style(styles.table.highlight);

        // Add title
        if let Some(title) = self.title {
            table = table.block(
                Block::default()
                    .title(title)
                    .title_style(styles.table.title),
            );
        }

        // Add optional header if given
        if let Some(header) = self.header {
            table = table.header(Row::new(header).style(styles.table.header));
        }

        // Defer to Ratatui's impl for the actual render
        StatefulWidget::render(table, area, buf, state);
    }
}
