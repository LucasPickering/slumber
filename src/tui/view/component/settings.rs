use crate::tui::{
    input::Action,
    view::{
        component::{
            Component, Draw, DrawContext, Event, Modal, Update, UpdateContext,
        },
        util::{Checkbox, ToTui},
    },
};
use derive_more::Display;
use ratatui::{
    prelude::{Constraint, Rect},
    widgets::{Cell, Row, Table, TableState},
};
use std::{cell::RefCell, ops::DerefMut};
use tracing::error;

/// Modal to view and modify user/view configuration
#[derive(Debug, Display)]
#[display(fmt = "SettingsModal")]
pub struct SettingsModal {
    table_state: RefCell<TableState>,
}

impl Default for SettingsModal {
    fn default() -> Self {
        Self {
            table_state: RefCell::new(
                TableState::default().with_selected(Some(0)),
            ),
        }
    }
}

impl Modal for SettingsModal {
    fn title(&self) -> &str {
        "Settings"
    }

    fn dimensions(&self) -> (Constraint, Constraint) {
        (Constraint::Length(30), Constraint::Length(5))
    }
}

impl Component for SettingsModal {
    fn update(&mut self, context: &mut UpdateContext, event: Event) -> Update {
        let table_state = self.table_state.get_mut();
        match event {
            Event::Input {
                action: Some(action),
                ..
            } => match action {
                // There are no other settings to scroll through yet, implement
                // that when necessary
                Action::Up => Update::Consumed,
                Action::Down => Update::Consumed,
                Action::Submit => {
                    match table_state.selected() {
                        Some(0) => {
                            context.config.preview_templates =
                                !context.config.preview_templates;
                        }
                        other => {
                            // Huh?
                            error!(
                                state = ?other,
                                "Unexpected settings table select state"
                            );
                        }
                    }
                    Update::Consumed
                }
                _ => Update::Propagate(event),
            },
            _ => Update::Propagate(event),
        }
    }
}

impl Draw for SettingsModal {
    fn draw(&self, context: &mut DrawContext, _: (), chunk: Rect) {
        let preview_templates_checkbox = Checkbox {
            checked: context.config.preview_templates,
        };
        let rows = vec![Row::new(vec![
            Cell::from("Preview Templates"),
            preview_templates_checkbox.to_tui(context).into(),
        ])];
        let table = Table::new(rows)
            .style(context.theme.table_text_style)
            .highlight_style(context.theme.table_highlight_style)
            .widths(&[Constraint::Percentage(80), Constraint::Percentage(20)]);

        context.frame.render_stateful_widget(
            table,
            chunk,
            self.table_state.borrow_mut().deref_mut(),
        );
    }
}
