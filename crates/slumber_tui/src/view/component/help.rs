use crate::{
    context::TuiContext,
    input::{Action, InputBinding},
    view::{
        common::{modal::Modal, table::Table},
        context::ViewContext,
        draw::{Draw, DrawMetadata, Generate},
        event::EventHandler,
    },
};
use itertools::Itertools;
use ratatui::{
    layout::{Alignment, Constraint, Layout},
    text::{Line, Text},
    Frame,
};
use slumber_core::{config::Config, util::TempDirectory};

const CRATE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// A mini helper in the footer for showing a few important key bindings
#[derive(Debug)]
pub struct HelpFooter;

impl Generate for HelpFooter {
    type Output<'this> = Text<'this>
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
            .filter_map(|action| {
                let binding = tui_context.input_engine.binding(action)?;
                Some(format!("{binding} {action}"))
            })
            .join(" / ");

        Text::styled(text, tui_context.styles.text.highlight)
    }
}

/// A whole ass modal for showing key binding help
#[derive(Debug, Default)]
pub struct HelpModal;

impl HelpModal {
    /// Number of lines in the general section (not including header)
    const GENERAL_LENGTH: u16 = 4;

    /// Get the list of bindings that will be shown in the modal
    fn bindings() -> impl Iterator<Item = (Action, &'static InputBinding)> {
        TuiContext::get()
            .input_engine
            .bindings()
            .iter()
            .filter(|(action, _)| action.visible())
            .map(|(action, binding)| (*action, binding))
    }
}

impl Modal for HelpModal {
    fn title(&self) -> Line<'_> {
        "Help".into()
    }

    fn dimensions(&self) -> (Constraint, Constraint) {
        let num_bindings = Self::bindings().count() as u16;
        (
            Constraint::Percentage(60),
            Constraint::Length(Self::GENERAL_LENGTH + 3 + num_bindings),
        )
    }
}

impl EventHandler for HelpModal {}

impl Draw for HelpModal {
    fn draw(&self, frame: &mut Frame, _: (), metadata: DrawMetadata) {
        // Create layout
        let [collection_area, _, keybindings_area] = Layout::vertical([
            Constraint::Length(Self::GENERAL_LENGTH + 1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .areas(metadata.area());

        // Collection metadata
        let collection_metadata = Table {
            title: Some("General"),
            rows: [
                ("Version", Line::from(CRATE_VERSION)),
                ("Configuration", Config::path().display().to_string().into()),
                (
                    "Log",
                    TempDirectory::get().log().display().to_string().into(),
                ),
                (
                    "Collection",
                    ViewContext::with_database(|database| {
                        database.collection_path()
                    })
                    .map(|path| path.display().to_string())
                    .unwrap_or_default()
                    .into(),
                ),
            ]
            .into_iter()
            .map(|(label, value)| {
                [Line::from(label), value.alignment(Alignment::Right)]
            })
            .collect(),
            column_widths: &[Constraint::Length(13), Constraint::Max(1000)],
            ..Default::default()
        };
        frame.render_widget(collection_metadata.generate(), collection_area);

        // Keybindings
        let keybindings = Table {
            title: Some("Keybindings"),
            rows: Self::bindings()
                .map(|(action, binding)| {
                    let action: Line = action.to_string().into();
                    let input: Line = binding.to_string().into();
                    [action, input.alignment(Alignment::Right)]
                })
                .collect_vec(),
            ..Default::default()
        };
        frame.render_widget(keybindings.generate(), keybindings_area);
    }
}
