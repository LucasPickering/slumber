use super::{Canvas, DrawMetadata};
use crate::view::{
    Generate, UpdateContext,
    common::{Pane, clear_fill::ClearFill},
    component::{Component, ComponentId, Draw},
    context::ViewContext,
    event::{Emitter, Event, EventMatch, ToEmitter},
};
use ratatui::{
    layout::{Alignment, Constraint, Layout},
    prelude::{Buffer, Rect},
    text::{Line, Span, Text},
    widgets::{Row, Table, Widget},
};
use slumber_config::{Action, Config, InputBinding};
use slumber_core::database::CollectionDatabase;
use slumber_util::{doc_link, paths};
use std::iter;

const CRATE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// A fullscreen help page
#[derive(Debug, Default)]
pub struct Help {
    id: ComponentId,
    emitter: Emitter<HelpEvent>,
}

impl Component for Help {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn update(&mut self, _: &mut UpdateContext, event: Event) -> EventMatch {
        event.m().action(|action, propagate| match action {
            Action::Cancel | Action::Quit | Action::Help => {
                self.emitter.emit(HelpEvent::Close);
            }
            _ => propagate.set(),
        })
    }
}

impl Draw for Help {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
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
        ]
        .into_iter()
        .map(|[label, value]| {
            [Span::styled(label, styles.table.header), value.into()]
        });
        let general_height = general_rows.len();
        let general = Table::new(
            general_rows.into_iter().map(Row::new),
            [13.into(), Constraint::Min(0)],
        );

        // Draw
        let block = Pane {
            title: "Help",
            has_focus: true,
        }
        .generate()
        .title(
            Line::from(format!(
                "{} to close",
                ViewContext::binding_display(Action::Cancel)
            ))
            .alignment(Alignment::Right),
        );
        let area = metadata.area();
        let [collection_area, keybindings_area] = Layout::vertical([
            Constraint::Length(general_height as u16),
            Constraint::Min(0),
        ])
        .areas(block.inner(area));
        canvas.render_widget(ClearFill, area);
        canvas.render_widget(block, area);
        canvas.render_widget(general, collection_area);
        canvas.render_widget(Keybindings, keybindings_area);
    }
}

impl ToEmitter<HelpEvent> for Help {
    fn to_emitter(&self) -> Emitter<HelpEvent> {
        self.emitter
    }
}

/// Emitted event for [Help]
#[derive(Debug)]
pub enum HelpEvent {
    Close,
}

/// Widget to display all key bindings
struct Keybindings;

impl Keybindings {
    /// Get input bindings grouped into similar sections
    fn groups() -> impl IntoIterator<Item = (&'static str, Vec<Action>)> {
        [
            (
                "Navigation",
                vec![
                    Action::Up,
                    Action::Down,
                    Action::Left,
                    Action::Right,
                    Action::ScrollUp,
                    Action::ScrollDown,
                    Action::ScrollLeft,
                    Action::ScrollRight,
                    Action::PageUp,
                    Action::PageDown,
                    Action::Home,
                    Action::End,
                ],
            ),
            (
                "Pane Navigation",
                vec![
                    Action::PreviousPane,
                    Action::NextPane,
                    Action::TopPane,
                    Action::BottomPane,
                    Action::Fullscreen,
                    Action::ToggleSidebar,
                    Action::ProfileList,
                    Action::RecipeList,
                    Action::History,
                    Action::Help,
                ],
            ),
            (
                "Interaction",
                vec![
                    Action::OpenActions,
                    Action::Submit,
                    Action::Toggle,
                    Action::Cancel,
                    Action::Delete,
                    Action::Edit,
                    Action::Reset,
                    Action::View,
                    Action::Search,
                    Action::Export,
                    Action::CommandHistory,
                ],
            ),
            (
                "Collection Management",
                vec![Action::SelectCollection, Action::ReloadCollection],
            ),
            ("Quittin' Time", vec![Action::Quit, Action::ForceQuit]),
        ]
    }
}

impl Widget for Keybindings {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let styles = ViewContext::styles();

        // Helper to generate a group header row
        let header_row = |group_name: &'static str| -> Row<'static> {
            Row::new([
                // Include a UNSTYLED padding line above the header
                Text::from_iter([
                    "".into(),
                    Span::styled(group_name, styles.table.header),
                ]),
                "".into(),
            ])
            .height(2) // Padding above
        };

        // For each group, generate a header row and all of its actions
        let rows =
            Self::groups()
                .into_iter()
                .flat_map(|(group_name, actions)| {
                    let binding_rows = actions.into_iter().map(|action| {
                        let binding = ViewContext::with_input(|input| {
                            input
                                .binding(action)
                                .map(InputBinding::to_string)
                                .unwrap_or_else(|| "<unbound>".to_owned())
                        });
                        Row::new([action.to_string(), binding])
                    });
                    iter::once(header_row(group_name)).chain(binding_rows)
                });
        let table = Table::new(rows, [21.into(), Constraint::Min(0)]);
        table.render(area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;
    use slumber_config::InputMap;
    use std::collections::HashSet;

    /// Make sure every bound action is visible in the Help page
    #[rstest]
    fn test_all_actions_shown() {
        let groups = Keybindings::groups();
        let shown_actions: Vec<_> = groups
            .into_iter()
            .flat_map(|(_, actions)| actions)
            .collect();
        let shown_actions_set = HashSet::from_iter(shown_actions.clone());
        let all_actions_set: HashSet<_> =
            InputMap::default().into_inner().into_keys().collect();
        assert_eq!(
            shown_actions_set, all_actions_set,
            "Help page is missing actions"
        );
        assert_eq!(
            shown_actions_set.len(),
            shown_actions.len(),
            "Help page has duplicate actions"
        );
    }
}
