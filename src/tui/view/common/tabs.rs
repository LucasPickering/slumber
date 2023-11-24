use crate::tui::{
    input::Action,
    view::{
        draw::{Draw, DrawContext},
        event::{Event, EventHandler, Update, UpdateContext},
        state::select::{Fixed, FixedSelect, SelectState},
        theme::Theme,
    },
};
use ratatui::prelude::Rect;
use std::fmt::Debug;

/// Multi-tab display. Generic parameter defines the available tabs.
#[derive(Debug, Default)]
pub struct Tabs<T: FixedSelect> {
    tabs: SelectState<Fixed, T, usize>,
}

impl<T: FixedSelect> Tabs<T> {
    pub fn selected(&self) -> &T {
        self.tabs.selected()
    }
}

impl<T: FixedSelect> EventHandler for Tabs<T> {
    fn update(&mut self, context: &mut UpdateContext, event: Event) -> Update {
        match event {
            Event::Input {
                action: Some(action),
                ..
            } => match action {
                Action::Left => {
                    self.tabs.previous(context);
                    Update::Consumed
                }
                Action::Right => {
                    self.tabs.next(context);
                    Update::Consumed
                }

                _ => Update::Propagate(event),
            },
            _ => Update::Propagate(event),
        }
    }
}

impl<T: FixedSelect> Draw for Tabs<T> {
    fn draw(&self, context: &mut DrawContext, _: (), area: Rect) {
        context.frame.render_widget(
            ratatui::widgets::Tabs::new(
                T::iter().map(|e| e.to_string()).collect(),
            )
            .select(self.tabs.selected_index())
            .highlight_style(Theme::get().tab_highlight_style),
            area,
        )
    }
}
