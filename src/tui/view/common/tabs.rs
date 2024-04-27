use crate::tui::{
    context::TuiContext,
    input::Action,
    view::{
        draw::Draw,
        event::{Event, EventHandler, Update},
        state::{
            fixed_select::{FixedSelect, FixedSelectState},
            persistence::{Persistable, Persistent, PersistentKey},
        },
    },
};
use ratatui::{prelude::Rect, Frame};
use std::fmt::Debug;

/// Multi-tab display. Generic parameter defines the available tabs.
#[derive(Debug)]
pub struct Tabs<T>
where
    T: FixedSelect + Persistable<Persisted = T>,
{
    tabs: Persistent<FixedSelectState<T, usize>>,
}

impl<T> Tabs<T>
where
    T: FixedSelect + Persistable<Persisted = T>,
{
    pub fn new(persistent_key: PersistentKey) -> Self {
        Self {
            tabs: Persistent::new(persistent_key, Default::default()),
        }
    }

    pub fn selected(&self) -> &T {
        self.tabs.selected()
    }
}

impl<T> EventHandler for Tabs<T>
where
    T: FixedSelect + Persistable<Persisted = T>,
{
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

impl<T> Draw for Tabs<T>
where
    T: FixedSelect + Persistable<Persisted = T>,
{
    fn draw(&self, frame: &mut Frame, _: (), area: Rect) {
        frame.render_widget(
            ratatui::widgets::Tabs::new(T::iter().map(|e| e.to_string()))
                .select(self.tabs.selected_index())
                .highlight_style(TuiContext::get().theme.tab.highlight),
            area,
        )
    }
}
