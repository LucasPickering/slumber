//! Profile selection and preview

use crate::{
    context::TuiContext,
    util::ResultReported,
    view::{
        Generate, ViewContext,
        common::{Pane, table::Table, template_preview::TemplatePreview},
        component::{
            Canvas, Component, ComponentId, Draw, DrawMetadata,
            sidebar_list::{SidebarListItem, SidebarListState},
        },
        persistent::PersistentKey,
    },
};
use anyhow::anyhow;
use indexmap::IndexMap;
use itertools::Itertools;
use serde::Serialize;
use slumber_config::Action;
use slumber_core::collection::{Profile, ProfileId};
use std::borrow::Cow;

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
    /// Precomputed field previews
    fields: Vec<(String, TemplatePreview)>,
}

impl ProfileDetail {
    /// Build the profile detail pane. This should be called whenever the
    /// selected profile changes, because the entire contents of the pane
    /// changes too.
    pub fn new(profile_id: Option<&ProfileId>) -> Self {
        let collection = ViewContext::collection();
        let default = IndexMap::new();
        let profile_data = profile_id
            .and_then(|profile_id| {
                let profile = collection
                    .profiles
                    .get(profile_id)
                    // Failure is a logic error
                    .ok_or_else(|| anyhow!("No profile with ID `{profile_id}`"))
                    .reported(&ViewContext::messages_tx())?;
                Some(&profile.data)
            })
            .unwrap_or(&default);

        // Start a preview render for each field
        let fields = profile_data
            .iter()
            .map(|(key, template)| {
                (
                    key.clone(),
                    TemplatePreview::new(
                        template.clone(),
                        None,
                        false,
                        // We don't know how this value will be used, so
                        // let's say we *do*
                        // support streaming to prevent loading
                        // some huge streams
                        true,
                    ),
                )
            })
            .collect_vec();

        Self {
            id: ComponentId::new(),
            fields,
        }
    }
}

impl Component for ProfileDetail {
    fn id(&self) -> ComponentId {
        self.id
    }
}

impl Draw for ProfileDetail {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        let title = TuiContext::get()
            .input_engine
            .add_hint("Profile", Action::SelectBottomPane);
        let block = Pane {
            title: &title,
            has_focus: metadata.has_focus(),
        }
        .generate();
        let area = block.inner(metadata.area());
        canvas.render_widget(block, metadata.area());

        let table = Table {
            header: Some(["Field", "Value"]),
            rows: self
                .fields
                .iter()
                .map(|(key, value)| [key.as_str().into(), value.generate()])
                .collect_vec(),
            alternate_row_style: true,
            ..Default::default()
        };
        canvas.render_widget(table, area);
    }
}
