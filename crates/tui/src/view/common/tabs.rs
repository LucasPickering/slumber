use crate::view::{
    common::fixed_select::{FixedSelect, FixedSelectBuilder, FixedSelectItem},
    component::{Canvas, Component, ComponentId, Draw, DrawMetadata},
    context::{UpdateContext, ViewContext},
    event::{Event, EventMatch},
    persistent::{PersistentKey, PersistentStore},
};
use ratatui::{style::Style, text::Line};
use slumber_config::Action;
use std::fmt::Debug;

/// Multi-tab display
/// - `K` is the key under which the selected tab is persisted. All tabs selects
///   persist their state!
/// - `T` is the tab enum
///
/// We store the key in here to limit the boilerplate that parents need to
/// restore and persist the state.
#[derive(Debug, Default)]
pub struct Tabs<K, T: FixedSelectItem> {
    id: ComponentId,
    persistent_key: K,
    select: FixedSelect<T, usize>,
}

impl<K: PersistentKey<Value = T>, T: FixedSelectItem> Tabs<K, T> {
    pub fn new(
        persistent_key: K,
        tabs_builder: FixedSelectBuilder<T, usize>,
    ) -> Self {
        let tabs = tabs_builder.persisted(&persistent_key).build();
        Self {
            id: ComponentId::default(),
            persistent_key,
            select: tabs,
        }
    }

    pub fn selected(&self) -> T {
        self.select.selected()
    }
}

impl<K: PersistentKey<Value = T>, T: FixedSelectItem> Component for Tabs<K, T> {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn update(&mut self, _: &mut UpdateContext, event: Event) -> EventMatch {
        event.m().action(|action, propagate| match action {
            Action::Left => self.select.previous(),
            Action::Right => self.select.next(),
            _ => propagate.set(),
        })
    }

    fn persist(&self, store: &mut PersistentStore) {
        store.set(&self.persistent_key, &self.selected());
    }
}

impl<K: PersistentKey<Value = T>, T: FixedSelectItem> Draw for Tabs<K, T> {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        let styles = ViewContext::styles().tab;
        let titles = self.select.items_with_metadata().map(|item| {
            let style = if item.enabled() {
                Style::default()
            } else {
                styles.disabled
            };
            Line::styled(item.value.to_string(), style)
        });
        canvas.render_widget(
            ratatui::widgets::Tabs::new(titles)
                .select(self.select.selected_index())
                .highlight_style(styles.highlight),
            metadata.area(),
        );
    }
}
