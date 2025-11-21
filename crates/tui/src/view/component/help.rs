use super::{Canvas, DrawMetadata};
use crate::{
    context::TuiContext,
    view::{
        Generate, UpdateContext,
        common::Pane,
        component::{Component, ComponentId, Draw},
        context::ViewContext,
        event::{Event, EventMatch},
    },
};
use itertools::Itertools;
use ratatui::{
    layout::{Alignment, Constraint, Layout},
    text::{Line, Span},
    widgets::{Row, Table},
};
use slumber_config::{Action, Config};
use slumber_core::database::CollectionDatabase;
use slumber_util::{doc_link, paths};
use unicode_width::UnicodeWidthStr;

const CRATE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// A mini helper in the footer for showing a few important key bindings
#[derive(Debug)]
pub struct HelpFooter;

impl Generate for HelpFooter {
    type Output<'this>
        = Span<'this>
    where
        Self: 'this;

    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this,
    {
        let actions = [Action::OpenActions, Action::OpenHelp, Action::Quit];

        let tui_context = TuiContext::get();

        let text = actions
            .into_iter()
            .map(|action| {
                let binding = tui_context.input_engine.binding_display(action);
                format!("{binding} {action}")
            })
            .join(" / ");

        Span::styled(text, tui_context.styles.text.highlight)
    }
}

/// A fullscreen help page, with metadata about the app as well as all
/// keybindings
///
/// This component maintains its own open/close state. The parent is responsible
/// for opening it (listening to the OpenHelp action), but it closes itself.
/// When this is open, the parent should draw it **and nothing else**. This
/// minimizes the amount of state/listening the parent has to do. We can't
/// listening to the open action in here though because we won't receive events
/// while closed.
#[derive(Debug, Default)]
pub struct Help {
    id: ComponentId,
    open: bool,
}

impl Help {
    /// Show the help page
    pub fn open(&mut self) {
        self.open = true;
    }

    /// Should the page be drawn?
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Get the list of bindings that will be shown in the modal
    fn bindings() -> Vec<[String; 2]> {
        TuiContext::get()
            .input_engine
            .bindings()
            .iter()
            .filter(|(action, _)| action.visible())
            .map(|(action, binding)| [action.to_string(), binding.to_string()])
            // Sort alphabetically
            .sorted_by_key(|[action, _]| action.to_string())
            .collect()
    }
}

impl Component for Help {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn update(
        &mut self,
        _context: &mut UpdateContext,
        event: Event,
    ) -> EventMatch {
        event.m().any(|event| match event {
            Event::Input(_) => {
                self.open = false;
                None
            }
            _ => Some(event),
        })
    }
}

impl Draw for Help {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        let styles = &TuiContext::get().styles;

        // General info/metadata
        let doc_link = doc_link("");
        let config_path = Config::path().display().to_string();
        let log_path = paths::log_file().display().to_string();
        let collection_path =
            ViewContext::with_database(CollectionDatabase::metadata)
                .map(|metadata| metadata.path.display().to_string())
                .unwrap_or_default();
        let general_rows = [
            ["Version", CRATE_VERSION],
            ["Docs", &doc_link],
            ["Configuration", &config_path],
            ["Log", &log_path],
            ["Collection", &collection_path],
        ];
        let general_height = general_rows.len();
        let general = Table::new(
            general_rows.into_iter().map(Row::new),
            [column_width(general_rows, 0), Constraint::Min(0)],
        );

        // Keybindings
        let keybindings = Self::bindings();
        let left_column_width = column_width(
            keybindings
                .iter()
                .map(|[action, binding]| [action.as_str(), binding.as_str()]),
            0,
        );
        let keybindings = Table::new(
            keybindings.into_iter().map(Row::new),
            [left_column_width, Constraint::Min(0)],
        )
        .header(Row::new(["Keybindings"]).style(styles.table.header));

        // Draw
        let block = Pane {
            title: "Help",
            has_focus: true,
        }
        .generate()
        .title(
            Line::from("Press any key to close").alignment(Alignment::Right),
        );
        let [collection_area, _, keybindings_area] = Layout::vertical([
            Constraint::Length(general_height as u16 + 1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .areas(block.inner(metadata.area()));
        canvas.render_widget(block, metadata.area());
        canvas.render_widget(general, collection_area);
        canvas.render_widget(keybindings, keybindings_area);
    }
}

/// Get the width of the widest item in a column
fn column_width<'a>(
    rows: impl IntoIterator<Item = [&'a str; 2]>,
    column: usize,
) -> Constraint {
    let width = rows
        .into_iter()
        .map(|row| row[column].width())
        .max()
        .unwrap_or(0);
    Constraint::Length(width as u16)
}
