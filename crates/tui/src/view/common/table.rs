use crate::{
    context::TuiContext,
    view::{common::Checkbox, draw::Generate},
};
use itertools::Itertools;
use ratatui::{
    prelude::Constraint,
    style::Styled,
    text::Text,
    widgets::{Block, Cell, Row},
};
use std::{iter, marker::PhantomData};

/// Tabular data display with a static number of columns.
///
/// The `R` generic defines the row type, which should be either an array of
/// cell types (e.g. `[Text; 3]`) or [ratatui::widgets::Row]. If using an array,
/// the length should match `COLS`. Allowing `Row` makes it possible to override
/// styling on a row-by-row basis.
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

impl<'a, const COLS: usize, Rows: Default> Default for Table<'a, COLS, Rows> {
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

impl<'a, const COLS: usize, Cll> Generate for Table<'a, COLS, [Cll; COLS]>
where
    Cll: Into<Cell<'a>>,
{
    type Output<'this> = ratatui::widgets::Table<'this>
    where
        Self: 'this;

    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this,
    {
        let table = Table {
            title: self.title,
            alternate_row_style: self.alternate_row_style,
            header: self.header,
            column_widths: self.column_widths,
            rows: self.rows.into_iter().map(Row::new).collect_vec(),
        };
        table.generate()
    }
}

impl<'a, const COLS: usize> Generate for Table<'a, COLS, Row<'a>> {
    type Output<'this> = ratatui::widgets::Table<'this>
    where
        Self: 'this;

    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this,
    {
        let styles = &TuiContext::get().styles;
        let rows = self.rows.into_iter().enumerate().map(|(i, row)| {
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
            .highlight_style(styles.table.highlight);

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

        table
    }
}

/// A row in a table that can be toggled on/off. This will generate the checkbox
/// column, and apply the appropriate row styling.
#[derive(Debug)]
pub struct ToggleRow<'a, Cells> {
    /// Needed to attach the lifetime of this value to the lifetime of the
    /// generated row
    phantom: PhantomData<&'a ()>,
    cells: Cells,
    enabled: bool,
}

impl<'a, Cells> ToggleRow<'a, Cells> {
    pub fn new(cells: Cells, enabled: bool) -> Self {
        Self {
            phantom: PhantomData,
            cells,
            enabled,
        }
    }
}

impl<'a, Cells> Generate for ToggleRow<'a, Cells>
where
    Cells: IntoIterator,
    Cells::Item: Into<Text<'a>>,
{
    type Output<'this> = Row<'this>
    where
        Self: 'this;

    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this,
    {
        let styles = &TuiContext::get().styles;
        // Include the given cells, then tack on the checkbox for enabled state
        Row::new(
            iter::once(
                Checkbox {
                    checked: self.enabled,
                }
                .generate()
                .into(),
            )
            .chain(self.cells.into_iter().map(Cell::from)),
        )
        .style(if self.enabled {
            styles.table.text
        } else {
            styles.table.disabled
        })
    }
}
