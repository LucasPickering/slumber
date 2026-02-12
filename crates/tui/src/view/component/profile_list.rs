use crate::view::{
    Generate, ToStringGenerate, UpdateContext, ViewContext,
    common::{
        Pane,
        select::{FilterItem, Select, SelectEventKind, SelectListProps},
        text_box::{TextBox, TextBoxEvent, TextBoxProps},
    },
    component::{
        Canvas, Child, Component, ComponentId, Draw, DrawMetadata, ToChild,
        misc::{SidebarEvent, SidebarFormat, SidebarProps},
    },
    event::{BroadcastEvent, Emitter, Event, EventMatch, ToEmitter},
    persistent::{PersistentKey, PersistentStore},
};
use derive_more::Display;
use ratatui::{
    layout::{Constraint, Layout},
    text::Text,
};
use serde::Serialize;
use slumber_config::Action;
use slumber_core::collection::{Profile, ProfileId};
use std::borrow::Cow;

/// Selectable list of profiles
#[derive(Debug)]
pub struct ProfileList {
    id: ComponentId,
    /// Emitter for open/close events
    emitter: Emitter<SidebarEvent>,
    /// Profile list
    select: Select<ProfileListItem>,
    /// Text box for filtering down items in the list
    filter: TextBox,
    /// Is the user typing in the filter box? User has to explicitly grab focus
    /// on the box to start typing
    filter_focused: bool,
}
impl ProfileList {
    pub fn new() -> Self {
        let filter = TextBox::default()
            .placeholder(format!(
                "{binding} to filter",
                binding = ViewContext::binding_display(Action::Search)
            ))
            .subscribe([
                TextBoxEvent::Cancel,
                TextBoxEvent::Change,
                TextBoxEvent::Submit,
            ]);
        let select = Self::build_select(filter.text());

        Self {
            id: ComponentId::default(),
            emitter: Emitter::default(),
            select,
            filter,
            filter_focused: false,
        }
    }

    /// ID of the selected profile, or `None` if the list is empty
    pub fn selected_id(&self) -> Option<&ProfileId> {
        self.select.selected().map(|item| &item.id)
    }

    /// Rebuild the select list based on filter state
    fn rebuild_select(&mut self) {
        self.select = Self::build_select(self.filter.text());
    }

    /// Build/rebuild a select based on the item list
    fn build_select(filter: &str) -> Select<ProfileListItem> {
        let profiles = &ViewContext::collection().profiles;
        let items = profiles.values().map(ProfileListItem::new).collect();

        Select::builder(items)
            .subscribe([SelectEventKind::Select, SelectEventKind::Submit])
            .filter(filter)
            .persisted(&SelectedProfileKey)
            .build()
    }
}

impl Component for ProfileList {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn update(&mut self, _: &mut UpdateContext, event: Event) -> EventMatch {
        event
            .m()
            .click(|_, _| self.emitter.emit(SidebarEvent::Open))
            .action(|action, propagate| match action {
                Action::Search => self.filter_focused = true,

                _ => propagate.set(),
            })
            // Emitted events from select
            .emitted(self.select.to_emitter(), |event| match event.kind {
                SelectEventKind::Select => {
                    // Let everyone know the selected profile changed
                    ViewContext::push_message(BroadcastEvent::SelectedProfile(
                        self.selected_id().cloned(),
                    ));
                }
                // Close the list on Enter
                SelectEventKind::Submit => {
                    self.emitter.emit(SidebarEvent::Close);
                }
                SelectEventKind::Toggle => {}
            })
            // Emitted events from filter
            .emitted(self.filter.to_emitter(), |event| match event {
                TextBoxEvent::Change => self.rebuild_select(),
                TextBoxEvent::Cancel | TextBoxEvent::Submit => {
                    self.filter_focused = false;
                }
            })
    }

    fn persist(&self, store: &mut PersistentStore) {
        store.set_opt(&SelectedProfileKey, self.selected_id());
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        vec![self.select.to_child_mut(), self.filter.to_child_mut()]
    }
}

impl Draw<SidebarProps> for ProfileList {
    fn draw(
        &self,
        canvas: &mut Canvas,
        props: SidebarProps,
        metadata: DrawMetadata,
    ) {
        // Both formats use a pane outline
        let title =
            ViewContext::add_binding_hint("Profile", Action::ProfileList);
        let block = Pane {
            title: &title,
            has_focus: metadata.has_focus(),
        }
        .generate();
        let area = block.inner(metadata.area());
        canvas.render_widget(block, metadata.area());

        match props.format {
            SidebarFormat::Header => {
                let value: Text = self
                    .select
                    .selected()
                    .map(|item| item.name.as_str().into())
                    .unwrap_or_else(|| "None".into());
                canvas.render_widget(value, area);
            }
            SidebarFormat::List => {
                // Expanded sidebar
                let [filter_area, list_area] = Layout::vertical([
                    Constraint::Length(1),
                    Constraint::Min(0),
                ])
                .areas(area);
                canvas.draw(
                    &self.filter,
                    TextBoxProps::default(),
                    filter_area,
                    self.filter_focused,
                );
                canvas.draw(
                    &self.select,
                    SelectListProps::pane(),
                    list_area,
                    true,
                );
            }
        }
    }
}

impl ToEmitter<SidebarEvent> for ProfileList {
    fn to_emitter(&self) -> Emitter<SidebarEvent> {
        self.emitter
    }
}

/// Simplified version of [Profile], to be used in the display list. This
/// only stores whatever data is necessary to render the list
#[derive(Clone, Debug, Display)]
#[display("{name}")]
pub struct ProfileListItem {
    id: ProfileId,
    name: String,
}

impl ProfileListItem {
    fn new(profile: &Profile) -> Self {
        Self {
            id: profile.id.clone(),
            name: profile.name().to_owned(),
        }
    }
}

impl ToStringGenerate for ProfileListItem {}

// For row selection
impl PartialEq<ProfileId> for ProfileListItem {
    fn eq(&self, id: &ProfileId) -> bool {
        &self.id == id
    }
}

impl FilterItem for ProfileListItem {
    fn search_terms(&self) -> impl IntoIterator<Item = Cow<'_, str>> {
        [(&self.id as &str).into(), self.name.as_str().into()]
    }
}

/// Persistent key for the ID of the selected profile
#[derive(Debug, Serialize)]
pub struct SelectedProfileKey;

impl PersistentKey for SelectedProfileKey {
    // Intentionally don't persist None. That's only possible if the profile map
    // is empty. If it is, we're forced into None. If not, we want to default to
    // the first profile.
    type Value = ProfileId;
}
