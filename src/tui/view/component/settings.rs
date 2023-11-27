use crate::tui::{
    context::TuiContext,
    message::Message,
    view::{
        common::{modal::Modal, table::Table, Checkbox},
        draw::{Draw, DrawContext, Generate},
        event::{EventHandler, UpdateContext},
        state::select::{Fixed, SelectState},
        Component, ViewConfig,
    },
};
use derive_more::Display;
use itertools::Itertools;
use ratatui::{
    prelude::{Constraint, Rect},
    widgets::{Cell, TableState},
};
use strum::{EnumCount, EnumIter, IntoEnumIterator};

/// Modal to view and modify user/view configuration
#[derive(Debug)]
pub struct SettingsModal {
    table: Component<SelectState<Fixed, Setting, TableState>>,
}

impl Default for SettingsModal {
    fn default() -> Self {
        // Toggle the selected setting on Enter
        let on_submit =
            |context: &mut UpdateContext, setting: &Setting| match setting {
                Setting::PreviewTemplates => {
                    context.config().preview_templates ^= true;
                }
                Setting::CaptureMouse => {
                    context.config().capture_mouse ^= true;
                    // Tell the terminal to actually do the switch
                    let capture = context.config().capture_mouse;
                    TuiContext::send_message(Message::ToggleMouseCapture {
                        capture,
                    });
                }
            };

        Self {
            table: SelectState::fixed().on_submit(on_submit).into(),
        }
    }
}

impl Modal for SettingsModal {
    fn title(&self) -> &str {
        "Settings"
    }

    fn dimensions(&self) -> (Constraint, Constraint) {
        (
            Constraint::Length(30),
            Constraint::Length(Setting::COUNT as u16 + 2),
        )
    }
}

impl EventHandler for SettingsModal {
    fn children(&mut self) -> Vec<Component<&mut dyn EventHandler>> {
        vec![self.table.as_child()]
    }
}

impl Draw for SettingsModal {
    fn draw(&self, context: &mut DrawContext, _: (), area: Rect) {
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
                column_widths: &[Constraint::Min(24), Constraint::Length(3)],
                ..Default::default()
            }
            .generate(),
            area,
            &mut self.table.state_mut(),
        );
    }
}

/// Various configurable settings
#[derive(
    Copy, Clone, Debug, Default, Display, EnumCount, EnumIter, PartialEq,
)]
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
