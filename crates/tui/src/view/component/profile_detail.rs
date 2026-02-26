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
            template_preview::{Preview, render_json_preview},
        },
        component::{
            Canvas, Child, Component, ComponentId, Draw, DrawMetadata, ToChild,
            editable_template::EditableTemplate,
        },
        persistent::{PersistentKey, PersistentStore, SessionKey},
    },
};
use anyhow::anyhow;
use async_trait::async_trait;
use indexmap::IndexMap;
use itertools::Itertools;
use ratatui::{
    layout::{Constraint, Layout, Spacing},
    style::Styled,
    text::Text,
};
use serde::Serialize;
use slumber_config::Action;
use slumber_core::{
    collection::{ProfileId, ValueTemplate},
    util::json::JsonTemplateError,
};
use slumber_template::Context;
use std::{borrow::Cow, iter, str::FromStr};
use unicode_width::UnicodeWidthStr;

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
                    ProfileTemplate(template.clone()),
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
    pub fn overrides(&self) -> IndexMap<String, ValueTemplate> {
        self.select
            .items()
            .filter_map(|field| {
                // Only include modified templates
                if field.template.is_overridden() {
                    Some((
                        field.field.clone(),
                        field.template.template().0.clone(),
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
        vec![self.select.to_child()]
    }
}

impl Draw for ProfileDetail {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        let title =
            ViewContext::add_binding_hint("Profile", Action::BottomPane);
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
                item_props: Box::new(move |item, _| {
                    (
                        ProfileFieldProps { field_column_width },
                        item.template.text().height() as u16,
                    )
                }),
            },
            rows_area,
            true,
        );
    }
}

/// A previewable wrapper of [ValueTemplate] for profile fields
#[derive(Clone, Debug, PartialEq)]
struct ProfileTemplate(ValueTemplate);

impl FromStr for ProfileTemplate {
    type Err = JsonTemplateError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        ValueTemplate::parse_json(s).map(Self)
    }
}

#[async_trait(?Send)]
impl Preview for ProfileTemplate {
    fn display(&self) -> Cow<'_, str> {
        // Convert to serde_json so we can offload formatting
        // TODO YAML
        let json: serde_json::Value = self.0.to_raw_json();
        format!("{json:#}").into()
    }

    fn is_dynamic(&self) -> bool {
        self.0.is_dynamic()
    }

    async fn render_preview<Ctx: Context>(
        &self,
        context: &Ctx,
    ) -> Text<'static> {
        // TODO YAML
        render_json_preview(context, &self.0).await
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
    type Value = ProfileTemplate;
}

/// A single field in the Profile detail table
#[derive(Debug)]
struct ProfileField {
    id: ComponentId,
    field: String,
    template: EditableTemplate<ProfileFieldOverrideKey, ProfileTemplate>,
}

impl ProfileField {
    fn new(
        profile_id: ProfileId,
        field: String,
        template: ProfileTemplate,
    ) -> Self {
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
        vec![self.template.to_child()]
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
    use crate::view::{
        event::BroadcastEvent,
        test_util::{TestComponent, TestHarness},
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
    fn test_edit_template() {
        let profile_id = ProfileId::from("profile1");
        let collection = Collection {
            profiles: by_id([Profile {
                id: profile_id.clone(),
                data: indexmap! {
                    "field1".into() => 12.into(),
                    "field2".into() => 34.into(),
                },
                ..Profile::factory(())
            }]),
            ..Collection::factory(())
        };
        let mut harness = TestHarness::new(collection);
        let mut component = TestComponent::new(
            &mut harness,
            ProfileDetail::new(Some(&profile_id)),
        );

        component
            .int(&mut harness)
            .send_keys([KeyCode::Down, KeyCode::Char('e')])
            .send_text("56")
            .send_key(KeyCode::Enter)
            // Tell all other previews to re-render
            .assert()
            .broadcast([BroadcastEvent::RefreshPreviews]);
        let field = &component.select[1];
        assert_eq!(field.template.template().0, 3456.into());
    }
}
