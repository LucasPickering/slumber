use super::{Canvas, DrawMetadata};
use crate::view::{
    Generate, UpdateContext,
    common::Pane,
    component::{Component, ComponentId, Draw},
    context::ViewContext,
    event::{Event, EventMatch},
};
use itertools::Itertools;
use ratatui::{
    layout::{Alignment, Constraint, Layout},
    text::{Line, Span},
    widgets::{Clear, Row, Table},
};
use slumber_config::{Action, Config};
use slumber_core::database::CollectionDatabase;
use slumber_util::{doc_link, paths};
use unicode_width::UnicodeWidthStr;

const CRATE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// A help footer that can be opened into a fullscreen help page
///
/// This manages its own open/close state and actions.
#[derive(Debug, Default)]
pub struct Help {
    id: ComponentId,
    /// If true, render on the entire frame. If false, show a summary in the
    /// footer
    open: bool,
}

impl Help {
    /// Get the list of bindings that will be shown in the modal
    fn bindings() -> Vec<[String; 2]> {
        ViewContext::with_input(|input| {
            input
                .bindings()
                .iter()
                .filter(|(action, _)| action.visible())
                .map(|(action, binding)| {
                    [action.to_string(), binding.to_string()]
                })
                // Sort alphabetically
                .sorted_by_key(|[action, _]| action.to_string())
                .collect()
        })
    }
}

impl Component for Help {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn update(&mut self, _: &mut UpdateContext, event: Event) -> EventMatch {
        event
            .m()
            .action(|action, propagate| match action {
                Action::OpenHelp if !self.open => self.open = true,
                _ => propagate.set(),
            })
            .any(|event| {
                // Any input exits fullscreen
                if let Event::Input(_) = event
                    && self.open
                {
                    self.open = false;
                    None
                } else {
                    Some(event)
                }
            })
    }
}

impl Draw for Help {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        if self.open {
            // Fullscreen mode is open
            let styles = ViewContext::styles();

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
                keybindings.iter().map(|[action, binding]| {
                    [action.as_str(), binding.as_str()]
                }),
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
                Line::from("Press any key to close")
                    .alignment(Alignment::Right),
            );
            let area = canvas.area(); // Use the whole dang screen
            let [collection_area, _, keybindings_area] = Layout::vertical([
                Constraint::Length(general_height as u16 + 1),
                Constraint::Length(1),
                Constraint::Min(0),
            ])
            .areas(block.inner(area));
            canvas.render_widget(Clear, area);
            canvas.render_widget(block, area);
            canvas.render_widget(general, collection_area);
            canvas.render_widget(keybindings, keybindings_area);
        } else {
            // Show minimal help in the footer
            let actions = [Action::OpenActions, Action::OpenHelp, Action::Quit];

            let text = actions
                .into_iter()
                .map(|action| {
                    let binding = ViewContext::binding_display(action);
                    format!("{binding} {action}")
                })
                .join(" / ");

            let span = Span::styled(text, ViewContext::styles().text.highlight)
                .into_right_aligned_line();
            canvas.render_widget(span, metadata.area());
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_util::{TestTerminal, terminal},
        view::test_util::{TestComponent, TestHarness, harness},
    };
    use rstest::rstest;
    use terminput::KeyCode;

    /// Open and close the help page
    #[rstest]
    fn test_open(harness: TestHarness, terminal: TestTerminal) {
        let mut component =
            TestComponent::new(&harness, &terminal, Help::default());
        assert!(!component.open);

        // Open help
        component
            .int()
            .send_key(KeyCode::Char('?'))
            .assert()
            .empty();
        assert!(component.open);

        // Any key should close. Events are *not* handled by anyone else
        component
            .int()
            .send_key(KeyCode::Char('x'))
            .assert()
            .empty();
        assert!(!component.open);
    }
}
