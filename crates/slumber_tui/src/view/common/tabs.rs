use crate::{
    context::TuiContext,
    input::Action,
    view::{
        draw::{Draw, DrawMetadata},
        event::{Event, EventHandler, Update},
        state::fixed_select::{FixedSelect, FixedSelectState},
    },
};
use persisted::PersistedContainer;
use ratatui::Frame;
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
    fn update(&mut self, event: Event) -> Update {
        let Some(action) = event.action() else {
            return Update::Propagate(event);
        };
        match action {
            Action::Left => self.tabs.previous(),
            Action::Right => self.tabs.next(),
            _ => return Update::Propagate(event),
        }
        Update::Consumed
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

    fn get_persisted(&self) -> Self::Value {
        self.tabs.get_persisted()
    }

    fn set_persisted(&mut self, value: Self::Value) {
        self.tabs.set_persisted(value)
    }
}
