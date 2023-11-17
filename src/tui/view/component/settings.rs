use crate::tui::{
    input::Action,
    message::Message,
    view::{
        common::{modal::Modal, table::Table, Checkbox},
        draw::{Draw, DrawContext, Generate},
        event::{Event, EventHandler, Update, UpdateContext},
        state::select::FixedSelectState,
        ViewConfig,
    },
};
use derive_more::Display;
use itertools::Itertools;
use ratatui::{
    prelude::{Constraint, Rect},
    widgets::{Cell, TableState},
};
use strum::{EnumIter, IntoEnumIterator};

/// Modal to view and modify user/view configuration
#[derive(Debug)]
pub struct SettingsModal {
    table: FixedSelectState<Setting, TableState>,
}

impl Default for SettingsModal {
    fn default() -> Self {
        Self {
            table: FixedSelectState::new(),
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

    fn as_component(&mut self) -> &mut dyn EventHandler {
        self
    }
}

impl EventHandler for SettingsModal {
    fn update(&mut self, context: &mut UpdateContext, event: Event) -> Update {
        match event {
            Event::Input {
                action: Some(Action::Submit),
                ..
            } => {
                match self.table.selected() {
                    Setting::PreviewTemplates => {
                        context.config().preview_templates ^= true;
                    }
                    Setting::CaptureMouse => {
                        context.config().capture_mouse ^= true;
                        let capture = context.config().capture_mouse;
                        context.send_message(Message::ToggleMouseCapture {
                            capture,
                        });
                    }
                }
                Update::Consumed
            }
            _ => Update::Propagate(event),
        }
    }

    fn children(&mut self) -> Vec<&mut dyn EventHandler> {
        vec![&mut self.table]
    }
}

impl Draw for SettingsModal {
    fn draw(&self, context: &mut DrawContext, _: (), chunk: Rect) {
        context.frame.render_stateful_widget(
            Table {
                rows: Setting::iter()
                    .map::<[Cell; 2], _>(|setting| {
                        [
                            setting.to_string().into(),
                            Checkbox {
                                checked: setting.get_value(context.config),
                            }
                            .generate()
                            .into(),
                        ]
                    })
                    .collect_vec(),
                alternate_row_style: false,
                column_widths: &[Constraint::Min(24), Constraint::Length(3)],
                ..Default::default()
            }
            .generate(),
            chunk,
            &mut self.table.state_mut(),
        );
    }
}

/// Various configurable settings
#[derive(Copy, Clone, Debug, Default, Display, EnumIter, PartialEq)]
enum Setting {
    #[default]
    #[display("Preview Templates")]
    PreviewTemplates,
    #[display("Capture Mouse")]
    CaptureMouse,
}

impl Setting {
    /// Get the value of a setting from the config
    fn get_value(self, config: &ViewConfig) -> bool {
        match self {
            Self::PreviewTemplates => config.preview_templates,
            Self::CaptureMouse => config.capture_mouse,
        }
    }
}
