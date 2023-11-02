use crate::tui::{
    input::Action,
    view::{
        component::{
            Component, Draw, DrawContext, Event, UpdateContext, UpdateOutcome,
        },
        state::{FixedSelect, StatefulSelect},
    },
};
use derive_more::Display;
use ratatui::prelude::Rect;
use std::fmt::Debug;

/// Multi-tab display. Generic parameter defines the available tabs.
#[derive(Debug, Default, Display)]
#[display(fmt = "Tabs")]
pub struct Tabs<T: FixedSelect> {
    tabs: StatefulSelect<T>,
}

impl<T: FixedSelect> Tabs<T> {
    pub fn selected(&self) -> T {
        self.tabs.selected()
    }
}

impl<T: Debug + FixedSelect> Component for Tabs<T> {
    fn update(
        &mut self,
        _context: &mut UpdateContext,
        event: Event,
    ) -> UpdateOutcome {
        match event {
            Event::Input {
                action: Some(action),
                ..
            } => match action {
                // Propagate TabChanged event if appropriate
                Action::Left => {
                    if self.tabs.previous() {
                        UpdateOutcome::Propagate(Event::TabChanged)
                    } else {
                        UpdateOutcome::Consumed
                    }
                }
                Action::Right => {
                    if self.tabs.next() {
                        UpdateOutcome::Propagate(Event::TabChanged)
                    } else {
                        UpdateOutcome::Consumed
                    }
                }

                _ => UpdateOutcome::Propagate(event),
            },
            _ => UpdateOutcome::Propagate(event),
        }
    }
}

impl<T: FixedSelect> Draw for Tabs<T> {
    fn draw(&self, context: &mut DrawContext, _: (), chunk: Rect) {
        context.frame.render_widget(
            ratatui::widgets::Tabs::new(
                T::iter().map(|e| e.to_string()).collect(),
            )
            .select(self.tabs.selected_index())
            .highlight_style(context.theme.text_highlight_style),
            chunk,
        )
    }
}
