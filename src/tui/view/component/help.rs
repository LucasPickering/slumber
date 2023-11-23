use crate::tui::{
    input::Action,
    view::{
        common::{modal::Modal, table::Table},
        draw::{Draw, DrawContext, Generate},
        event::EventHandler,
        theme::Theme,
    },
};
use itertools::Itertools;
use ratatui::{
    layout::{Alignment, Constraint, Rect},
    text::Line,
    widgets::Paragraph,
};

/// A mini helper in the footer for showing a few important key bindings
#[derive(Debug)]
pub struct HelpFooter;

impl Draw for HelpFooter {
    fn draw(&self, context: &mut DrawContext, _: (), chunk: Rect) {
        // Decide which actions to show based on context. This is definitely
        // spaghetti and easy to get out of sync, but it's the easiest way to
        // get granular control
        let actions = [Action::OpenSettings, Action::OpenHelp, Action::Quit];

        let text = actions
            .into_iter()
            .map(|action| {
                context
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
                .style(Theme::get().text_highlight),
            chunk,
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
        (Constraint::Length(30), Constraint::Length(11))
    }

    fn as_event_handler(&mut self) -> &mut dyn EventHandler {
        self
    }
}

impl EventHandler for HelpModal {}

impl Draw for HelpModal {
    fn draw(&self, context: &mut DrawContext, _: (), chunk: Rect) {
        let table = Table {
            rows: context
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
        context.frame.render_widget(table.generate(), chunk);
    }
}
