use crate::{
    util::ResultReported,
    view::{
        Generate, UpdateContext, ViewContext,
        common::{
            Pane,
            component_select::{
                ComponentSelect, ComponentSelectProps, SelectStyles,
            },
            select::Select,
        },
        component::{
            Canvas, Child, Component, ComponentId, Draw, DrawMetadata, ToChild,
            editable_template::EditableTemplate,
        },
        event::{BroadcastEvent, Event, EventMatch},
        persistent::{PersistentKey, PersistentStore, SessionKey},
    },
};
use anyhow::anyhow;
use indexmap::IndexMap;
use itertools::Itertools;
use ratatui::{
    layout::{Constraint, Layout, Spacing},
    style::Styled,
};
use serde::Serialize;
use slumber_config::Action;
use slumber_core::collection::ProfileId;
use slumber_template::Template;
use std::{collections::HashMap, iter};
use unicode_width::UnicodeWidthStr;

/// Preview the fields of a profile
#[derive(Debug)]
pub struct ProfileDetail {
    id: ComponentId,
    /// ID of the displayed profile. Set by [BroadcastEvent::SelectedProfile]
    selected_profile_id: Option<ProfileId>,
    /// Cache the rendered fields for profiles as they're selected. Profiles
    /// are never evicted because they're immutable (except for overrides,
    /// which are modified inline within the cache). This prevents the need to
    /// rerender the same templates over-and-over.
    profiles: HashMap<ProfileId, ComponentSelect<ProfileField>>,
}

impl ProfileDetail {
    pub fn new() -> Self {
        Self {
            id: ComponentId::new(),
            selected_profile_id: None,
            profiles: HashMap::new(),
        }
    }

    /// Get a map of overridden profile fields
    pub fn overrides(&self) -> IndexMap<String, Template> {
        let Some(select) = self.selected_profile() else {
            return IndexMap::new();
        };

        select
            .items()
            .filter_map(|field| {
                // Only include modified templates
                if field.template.is_overridden() {
                    Some((
                        field.field.clone(),
                        field.template.template().clone(),
                    ))
                } else {
                    None
                }
            })
            .collect()
    }

    /// Change the selected profile. If the new profile isn't already in the
    /// cache, render its fields now.
    fn select_profile(&mut self, profile_id: Option<ProfileId>) {
        self.selected_profile_id = profile_id;
        if let Some(profile_id) = &self.selected_profile_id
            && !self.profiles.contains_key(profile_id)
        {
            let select = Self::build_select(profile_id);
            self.profiles.insert(profile_id.clone(), select);
        }
    }

    /// Get a reference to the selected profile's field table
    fn selected_profile(&self) -> Option<&ComponentSelect<ProfileField>> {
        self.selected_profile_id
            .as_ref()
            .and_then(|id| self.profiles.get(id))
    }

    /// Build the field table for a profile
    fn build_select(profile_id: &ProfileId) -> ComponentSelect<ProfileField> {
        let collection = ViewContext::collection();
        let default = IndexMap::new();
        let profile_data = collection
            .profiles
            .get(profile_id)
            // Failure is a logic error
            .ok_or_else(|| anyhow!("No profile with ID `{profile_id}`"))
            .reported(&ViewContext::messages_tx())
            .map(|profile| &profile.data)
            .unwrap_or(&default);

        // Create an editable template for each field
        let items = profile_data
            .iter()
            .map(|(field, template)| {
                ProfileField::new(
                    profile_id.clone(),
                    field.clone(),
                    template.clone(),
                )
            })
            .collect_vec();
        Select::builder(items)
            .persisted(&SelectedProfileFieldKey)
            .build()
            .into()
    }
}

impl Component for ProfileDetail {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn update(
        &mut self,
        _context: &mut UpdateContext,
        event: Event,
    ) -> EventMatch {
        event.m().broadcast(|event| match event {
            // Whenever the profile selection changes, update our state
            BroadcastEvent::SelectedProfile(profile_id) => {
                self.select_profile(profile_id);
            }
            _ => {}
        })
    }

    fn persist(&self, store: &mut PersistentStore) {
        // Persist selected row
        store.set_opt(
            &SelectedProfileFieldKey,
            self.selected_profile().and_then(|select| {
                let row = select.selected()?;
                Some(&row.field)
            }),
        );
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        // All cached profiles are accessible, so they can received their
        // preview callbacks even when not visible
        self.profiles
            .values_mut()
            .map(ToChild::to_child_mut)
            .collect()
    }
}

impl Draw for ProfileDetail {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        let title =
            ViewContext::add_binding_hint("Profile", Action::SelectBottomPane);
        let block = Pane {
            title: &title,
            has_focus: metadata.has_focus(),
        }
        .generate();
        let area = block.inner(metadata.area());
        canvas.render_widget(block, metadata.area());
        let Some(select) = self.selected_profile() else {
            // No empty state - maybe to be changed later?
            return;
        };

        // Find the widest field so we know how to size the field column
        let field_column_width = iter::once("Field")
            .chain(select.items().map(|row| row.field.as_str()))
            .map(UnicodeWidthStr::width)
            .max()
            .unwrap_or(0) as u16
            + 1; // Padding!

        let [header_area, rows_area] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(0)])
                .areas(area);
        let [key_header_area, value_header_area] = Layout::horizontal([
            Constraint::Length(field_column_width),
            Constraint::Min(1),
        ])
        .areas(header_area);

        // Draw header
        let style = ViewContext::styles().table.header;
        canvas.render_widget("Field".set_style(style), key_header_area);
        canvas.render_widget("Value".set_style(style), value_header_area);

        // Draw rows
        canvas.draw(
            select,
            ComponentSelectProps {
                styles: SelectStyles::table(),
                spacing: Spacing::default(),
                item_props: Box::new(move |_, _| {
                    (ProfileFieldProps { field_column_width }, 1)
                }),
            },
            rows_area,
            true,
        );
    }
}

/// Persistence key for selected row in the [ProfileDetail] table
#[derive(Debug, Serialize)]
struct SelectedProfileFieldKey;

impl PersistentKey for SelectedProfileFieldKey {
    /// Store the field name
    type Value = String;
}

/// Persistence key for overridden profile field template in the session store
#[derive(Debug, Clone, PartialEq)]
struct ProfileFieldOverrideKey {
    profile_id: ProfileId,
    field: String,
}

impl SessionKey for ProfileFieldOverrideKey {
    type Value = Template;
}

/// A single field in the Profile detail table
#[derive(Debug)]
struct ProfileField {
    id: ComponentId,
    field: String,
    template: EditableTemplate<ProfileFieldOverrideKey>,
}

impl ProfileField {
    fn new(profile_id: ProfileId, field: String, template: Template) -> Self {
        let template = EditableTemplate::new(
            "Field",
            ProfileFieldOverrideKey {
                profile_id,
                field: field.clone(),
            },
            template,
            // We don't know how this value will be used, so let's say we *do*
            // support streaming to prevent loading some huge streams
            true,
            // This edit could have downstream changes, so refresh after edit
            true,
        );
        Self {
            id: ComponentId::new(),
            field,
            template,
        }
    }
}

impl Component for ProfileField {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        vec![self.template.to_child_mut()]
    }
}

impl Draw<ProfileFieldProps> for ProfileField {
    fn draw(
        &self,
        canvas: &mut Canvas,
        props: ProfileFieldProps,
        metadata: DrawMetadata,
    ) {
        let [field_area, template_area] = Layout::horizontal([
            Constraint::Length(props.field_column_width),
            Constraint::Min(1),
        ])
        .areas(metadata.area());

        canvas.render_widget(self.field.as_str(), field_area);
        canvas.draw(&self.template, (), template_area, true);
    }
}

// Compare against field name for persistence
impl PartialEq<String> for ProfileField {
    fn eq(&self, other: &String) -> bool {
        &self.field == other
    }
}

/// Props for a single row in the field table
#[derive(Copy, Clone, Debug)]
struct ProfileFieldProps {
    /// Width of the left column
    field_column_width: u16,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_util::{TestTerminal, terminal},
        view::{
            event::BroadcastEvent,
            test_util::{TestComponent, TestHarness},
        },
    };
    use indexmap::indexmap;
    use rstest::rstest;
    use slumber_core::{
        collection::{Collection, Profile},
        test_util::by_id,
    };
    use slumber_util::Factory;
    use terminput::KeyCode;

    #[rstest]
    fn test_edit_template(terminal: TestTerminal) {
        let profile_id = ProfileId::from("profile1");
        let collection = Collection {
            profiles: by_id([Profile {
                id: profile_id.clone(),
                data: indexmap! {
                    "field1".into() => "abc".into(),
                    "field2".into() => "def".into(),
                },
                ..Profile::factory(())
            }]),
            ..Collection::factory(())
        };
        let harness = TestHarness::new(collection);
        let mut component =
            TestComponent::new(&harness, &terminal, ProfileDetail::new());

        let profile_id = Some(profile_id);
        component
            .int(&harness)
            // Emulate selecting the profile
            .send_event(BroadcastEvent::SelectedProfile(profile_id.clone()))
            .send_keys([KeyCode::Down, KeyCode::Char('e')])
            .send_text("123")
            .send_key(KeyCode::Enter)
            // Tell all other previews to re-render
            .assert()
            .broadcast([
                BroadcastEvent::SelectedProfile(profile_id),
                BroadcastEvent::RefreshPreviews,
            ]);
        let field = &component.selected_profile().unwrap()[1];
        assert_eq!(field.template.template(), &"def123".into());
    }
}
