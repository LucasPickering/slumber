use crate::tui::{
    context::TuiContext,
    input::Action,
    view::{
        common::{modal::Modal, table::Table},
        draw::{Draw, DrawContext, Generate},
        event::EventHandler,
        util::layout,
    },
};
use itertools::Itertools;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Rect},
    text::Line,
    widgets::Paragraph,
};

/// A mini helper in the footer for showing a few important key bindings
#[derive(Debug)]
pub struct HelpFooter;

impl Draw for HelpFooter {
    fn draw(&self, context: &mut DrawContext, _: (), area: Rect) {
        // Decide which actions to show based on context. This is definitely
        // spaghetti and easy to get out of sync, but it's the easiest way to
        // get granular control
        let actions = [Action::OpenSettings, Action::OpenHelp, Action::Quit];

        let tui_context = TuiContext::get();

        let text = actions
            .into_iter()
            .map(|action| {
                tui_context
                    .input_engine
                    .binding(action)
                    .as_ref()
                    .map(ToString::to_string)
                    // This *shouldn't* happen, all actions get a binding
                    .unwrap_or_else(|| "???".into())
            })
            .join(" / ");

        context.frame.render_widget(
            Paragraph::new(text)
                .alignment(Alignment::Right)
                .style(tui_context.theme.text_highlight),
            area,
        );
    }
}

/// A whole ass modal for showing key binding help
#[derive(Debug, Default)]
pub struct HelpModal;

impl Modal for HelpModal {
    fn title(&self) -> &str {
        "Help"
    }

    fn dimensions(&self) -> (Constraint, Constraint) {
        (Constraint::Percentage(80), Constraint::Length(16))
    }
}

impl EventHandler for HelpModal {}

impl Draw for HelpModal {
    fn draw(&self, context: &mut DrawContext, _: (), area: Rect) {
        let tui_context = TuiContext::get();

        // Create layout
        let [collection_area, _, keybindings_area] = layout(
            area,
            Direction::Vertical,
            [
                Constraint::Length(2),
                Constraint::Length(1),
                Constraint::Min(0),
            ],
        );

        // Collection metadata
        let collection_metadata = Table {
            title: Some("Collection"),
            rows: [[
                Line::from("Path"),
                Line::from(
                    tui_context
                        .database
                        .collection_path()
                        .map(|path| path.display().to_string())
                        .unwrap_or_default(),
                )
                .alignment(Alignment::Right),
            ]],
            column_widths: &[Constraint::Length(5), Constraint::Max(100)],
            ..Default::default()
        };
        context
            .frame
            .render_widget(collection_metadata.generate(), collection_area);

        // Keybindings
        let keybindings = Table {
            title: Some("Keybindings"),
            rows: tui_context
                .input_engine
                .bindings()
                .values()
                .filter(|binding| binding.visible())
                .map(|binding| {
                    let action: Line = binding.action().to_string().into();
                    let input: Line = binding.input().to_string().into();
                    [action, input.alignment(Alignment::Right)]
                })
                .collect_vec(),
            ..Default::default()
        };
        context
            .frame
            .render_widget(keybindings.generate(), keybindings_area);
    }
}
