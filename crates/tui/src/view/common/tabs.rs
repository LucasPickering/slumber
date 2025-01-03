use crate::{
    context::TuiContext,
    view::{
        context::UpdateContext,
        draw::{Draw, DrawMetadata},
        event::{Event, EventHandler, Update},
        state::fixed_select::{FixedSelect, FixedSelectState},
    },
};
use persisted::PersistedContainer;
use ratatui::Frame;
use slumber_config::Action;
use std::fmt::Debug;

/// Multi-tab display. Generic parameter defines the available tabs.
#[derive(Debug, Default)]
pub struct Tabs<T: FixedSelect> {
    tabs: FixedSelectState<T, usize>,
}

impl<T: FixedSelect> Tabs<T> {
    pub fn selected(&self) -> T {
        self.tabs.selected()
    }
}

impl<T: FixedSelect> EventHandler for Tabs<T> {
    fn update(&mut self, _: &mut UpdateContext, event: Event) -> Update {
        event.m().action(|action, propagate| match action {
            Action::Left => self.tabs.previous(),
            Action::Right => self.tabs.next(),
            _ => propagate.set(),
        })
    }
}

impl<T: FixedSelect> Draw for Tabs<T> {
    fn draw(&self, frame: &mut Frame, _: (), metadata: DrawMetadata) {
        frame.render_widget(
            ratatui::widgets::Tabs::new(T::iter().map(|e| e.to_string()))
                .select(self.tabs.selected_index())
                .highlight_style(TuiContext::get().styles.tab.highlight),
            metadata.area(),
        )
    }
}

/// Persist selected tab
impl<T> PersistedContainer for Tabs<T>
where
    T: FixedSelect,
{
    type Value = T;

    fn get_to_persist(&self) -> Self::Value {
        self.tabs.get_to_persist()
    }

    fn restore_persisted(&mut self, value: Self::Value) {
        self.tabs.restore_persisted(value)
    }
}
