use crate::tui::{
    context::TuiContext,
    input::Action,
    view::{
        draw::Draw,
        event::{Event, EventHandler, Update},
        state::{
            persistence::{Persistable, Persistent, PersistentKey},
            select::{Fixed, FixedSelect, SelectState},
        },
    },
};
use ratatui::{prelude::Rect, Frame};
use std::fmt::Debug;

/// Multi-tab display. Generic parameter defines the available tabs.
#[derive(Debug)]
pub struct Tabs<T>
where
    T: FixedSelect + Persistable,
    T::Persisted: PartialEq<T>,
{
    tabs: Persistent<SelectState<Fixed, T, usize>>,
}

impl<T> Tabs<T>
where
    T: FixedSelect + Persistable,
    T::Persisted: PartialEq<T>,
{
    pub fn new(persistent_key: PersistentKey) -> Self {
        Self {
            tabs: Persistent::new(persistent_key, SelectState::default()),
        }
    }

    pub fn selected(&self) -> &T {
        self.tabs.selected()
    }
}

impl<T> EventHandler for Tabs<T>
where
    T: FixedSelect + Persistable,
    T::Persisted: PartialEq<T>,
{
    fn update(&mut self, event: Event) -> Update {
        match event {
            Event::Input {
                action: Some(action),
                ..
            } => match action {
                Action::Left => {
                    self.tabs.previous();
                    Update::Consumed
                }
                Action::Right => {
                    self.tabs.next();
                    Update::Consumed
                }

                _ => Update::Propagate(event),
            },
            _ => Update::Propagate(event),
        }
    }
}

impl<T> Draw for Tabs<T>
where
    T: FixedSelect + Persistable,
    T::Persisted: PartialEq<T>,
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
