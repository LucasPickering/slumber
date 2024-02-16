use crate::tui::{
    context::TuiContext,
    input::Action,
    view::{
        draw::Draw,
        event::{Event, EventHandler, Update, UpdateContext},
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
pub struct Tabs<T: FixedSelect + Persistable> {
    tabs: Persistent<SelectState<Fixed, T, usize>>,
}

impl<T: FixedSelect + Persistable> Tabs<T> {
    pub fn new(persistent_key: PersistentKey) -> Self {
        Self {
            tabs: Persistent::new(persistent_key, SelectState::default()),
        }
    }

    pub fn selected(&self) -> &T {
        self.tabs.selected()
    }
}

impl<T: FixedSelect + Persistable> EventHandler for Tabs<T> {
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

impl<T: FixedSelect + Persistable> Draw for Tabs<T> {
    fn draw(&self, frame: &mut Frame, _: (), area: Rect) {
        frame.render_widget(
            ratatui::widgets::Tabs::new(T::iter().map(|e| e.to_string()))
                .select(self.tabs.selected_index())
                .highlight_style(TuiContext::get().theme.tab_highlight_style),
            area,
        )
    }
}
