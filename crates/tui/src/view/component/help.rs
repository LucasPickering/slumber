use super::{Canvas, DrawMetadata};
use crate::view::{
    Generate, UpdateContext,
    common::{
        Pane,
        text_window::{TextWindow, TextWindowProps},
    },
    component::{Child, Component, ComponentId, Draw, ToChild},
    context::ViewContext,
    event::{Emitter, Event, EventMatch, ToEmitter},
    styles::Styles,
};
use ratatui::{
    layout::Alignment,
    text::{Line, Span, Text},
};
use slumber_config::{Action, Config, InputBinding};
use slumber_core::database::CollectionDatabase;
use slumber_util::{doc_link, paths};

const CRATE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// A fullscreen help page
#[derive(Debug)]
pub struct Help {
    id: ComponentId,
    emitter: Emitter<HelpEvent>,
    /// Scrollable text window. All text is static and pregenerated
    text_window: TextWindow,
}

impl Default for Help {
    fn default() -> Self {
        Self {
            id: ComponentId::new(),
            emitter: Emitter::default(),
            text_window: TextWindow::new(help_text()),
        }
    }
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

    fn children(&mut self) -> Vec<Child<'_>> {
        vec![self.text_window.to_child()]
    }
}

impl Draw for Help {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
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
        canvas.render_widget(&block, area);
        canvas.draw(
            &self.text_window,
            TextWindowProps {
                line_numbers: false,
                ..TextWindowProps::default()
            },
            block.inner(area),
            true,
        );
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

/// Generate the help text
///
/// Content is generated as Text instead of tables so we can use it with
/// TextWindow. Text makes it easy to calculate the total size, for scrolling.
fn help_text() -> Text<'static> {
    let mut text = Text::default();
    let styles = ViewContext::styles();
    text.extend(aligned(general_rows(&styles)));
    text.extend(aligned(input_rows(&styles)));
    text
}

fn general_rows(styles: &Styles) -> Vec<[Span<'static>; 2]> {
    let doc_link = doc_link("");
    let config_path = Config::path().display().to_string();
    let log_path = paths::log_file().display().to_string();
    let collection_path =
        ViewContext::with_database(CollectionDatabase::metadata)
            .map(|metadata| metadata.path.display().to_string())
            .unwrap_or_default();
    vec![
        [header(styles, "Version"), CRATE_VERSION.into()],
        [header(styles, "Docs"), doc_link.into()],
        [header(styles, "Configuration"), config_path.into()],
        [header(styles, "Log"), log_path.into()],
        [header(styles, "Collection"), collection_path.into()],
    ]
}

/// Get text rows for all
fn input_rows(styles: &Styles) -> Vec<[Span<'static>; 2]> {
    // For each group, generate a header line and all of its actions
    input_groups()
        .into_iter()
        .flat_map(|(group_name, actions)| {
            let binding_rows = actions.into_iter().map(|action| {
                let binding = ViewContext::with_input(|input| {
                    input
                        .binding(action)
                        .map(InputBinding::to_string)
                        .unwrap_or_else(|| "<unbound>".to_owned())
                });
                [action.to_string().into(), binding.into()]
            });
            // Include a padding line between groups
            [
                [Span::default(), Span::default()],
                [header(styles, group_name), "".into()],
            ]
            .into_iter()
            .chain(binding_rows)
        })
        .collect()
}

/// Apply header styles to text
fn header<'s>(styles: &Styles, label: &'s str) -> Span<'s> {
    Span::styled(label, styles.table.header)
}

/// Add padding to the first column of each row to align the second column
fn aligned(
    rows: Vec<[Span<'static>; 2]>,
) -> impl IntoIterator<Item = Line<'static>> {
    // Include +1 for guaranteed padding
    let left_column_width =
        rows.iter().map(|[left, _]| left.width()).max().unwrap_or(0) + 1;
    // Add a padding span to the middle of each line
    rows.into_iter().map(move |[left, right]| {
        // Safety: left_column_width is >= to all cell widths, so no underflow
        let padding_width = left_column_width - left.width();
        vec![left, " ".repeat(padding_width).into(), right].into()
    })
}

/// Get input bindings grouped into similar sections
fn input_groups() -> impl IntoIterator<Item = (&'static str, Vec<Action>)> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;
    use slumber_config::InputMap;
    use std::collections::HashSet;

    /// Make sure every bound action is visible in the Help page
    #[rstest]
    fn test_all_actions_shown() {
        let groups = input_groups();
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
