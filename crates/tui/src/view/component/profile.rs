//! Profile selection and preview

use crate::{
    util::ResultReported,
    view::{
        Generate, ViewContext,
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
            sidebar_list::{SidebarListItem, SidebarListState},
        },
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
use slumber_core::collection::{Profile, ProfileId};
use slumber_template::Template;
use std::{borrow::Cow, iter};
use unicode_width::UnicodeWidthStr;

/// State for a list of profiles. Use with
/// [SidebarList](super::sidebar_list::SidebarList) for display.
#[derive(Debug, Default)]
pub struct ProfileListState;

impl SidebarListState for ProfileListState {
    const TITLE: &str = "Profile";
    const ACTION: Action = Action::SelectProfileList;
    type Item = ProfileListItem;
    type PersistentKey = SelectedProfileKey;

    fn persistent_key(&self) -> Self::PersistentKey {
        SelectedProfileKey
    }

    fn items(&self) -> Vec<Self::Item> {
        ViewContext::collection()
            .profiles
            .values()
            .map(ProfileListItem::new)
            .collect()
    }
}

/// Simplified version of [Profile], to be used in the display list. This
/// only stores whatever data is necessary to render the list
#[derive(Clone, Debug)]
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

impl SidebarListItem for ProfileListItem {
    type Id = ProfileId;

    fn id(&self) -> &Self::Id {
        &self.id
    }

    fn display_header(&self) -> Cow<'_, str> {
        self.name.as_str().into()
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

/// Preview the fields of a profile
#[derive(Debug)]
pub struct ProfileDetail {
    id: ComponentId,
    /// Navigable list of profile fields
    select: ComponentSelect<ProfileField>,
}

impl ProfileDetail {
    /// Build the profile detail pane. This should be called whenever the
    /// selected profile changes, because the entire contents of the pane
    /// changes too.
    pub fn new(profile_id: Option<&ProfileId>) -> Self {
        let Some(profile_id) = profile_id else {
            // No profile selected - empty state
            return Self {
                id: ComponentId::new(),
                select: ComponentSelect::default(),
            };
        };

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
        let select = Select::builder(items)
            .persisted(&SelectedProfileFieldKey)
            .build()
            .into();

        Self {
            id: ComponentId::new(),
            select,
        }
    }

    /// Get a map of overridden profile fields
    pub fn overrides(&self) -> IndexMap<String, Template> {
        self.select
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
}

impl Component for ProfileDetail {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn persist(&self, store: &mut PersistentStore) {
        // Persist selected row
        store.set_opt(
            &SelectedProfileFieldKey,
            self.select.selected().map(|row| &row.field),
        );
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        vec![self.select.to_child_mut()]
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

        // Find the widest field so we know how to size the field column
        let field_column_width = iter::once("Field")
            .chain(self.select.items().map(|row| row.field.as_str()))
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
            &self.select,
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
    use slumber_core::{collection::Collection, test_util::by_id};
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
        let mut component = TestComponent::new(
            &harness,
            &terminal,
            ProfileDetail::new(Some(&profile_id)),
        );

        component
            .int()
            .send_keys([KeyCode::Down, KeyCode::Char('e')])
            .send_text("123")
            .send_key(KeyCode::Enter)
            // Tell all other previews to re-render
            .assert()
            .broadcast([BroadcastEvent::RefreshPreviews]);
        let field = &component.select[1];
        assert_eq!(field.template.template(), &"def123".into());
    }
}
