//! Profile selection and preview

use crate::{
    context::TuiContext,
    util::ResultReported,
    view::{
        Component, Generate, ViewContext,
        common::{Pane, table::Table, template_preview::TemplatePreview},
        component::{
            Canvas, ComponentId, Draw, DrawMetadata,
            primary_list::PrimaryListState,
        },
        state::StateCell,
    },
};
use anyhow::anyhow;
use itertools::Itertools;
use ratatui::text::{Span, Text};
use slumber_config::Action;
use slumber_core::collection::{HasId, Profile, ProfileId};
use slumber_util::doc_link;

/// TODO
#[derive(Debug)]
pub struct ProfileListState;

impl PrimaryListState for ProfileListState {
    const TITLE: &str = "Profile";
    const ACTION: Action = Action::SelectProfileList;
    type Item = ProfileListItem;

    fn items(&self) -> Vec<Self::Item> {
        ViewContext::collection()
            .profiles
            .values()
            .map(Self::Item::from)
            .collect()
    }

    fn empty_text(&self) -> Text<'static> {
        Text::from(vec![
            "No profiles defined; add one to your collection.".into(),
            doc_link("api/request_collection/profile").into(),
        ])
    }
}

/// Simplified version of [Profile], to be used in the display list. This
/// only stores whatever data is necessary to render the list
#[derive(Clone, Debug)]
pub struct ProfileListItem {
    id: ProfileId,
    name: String,
}

impl HasId for ProfileListItem {
    type Id = ProfileId;

    fn id(&self) -> &Self::Id {
        &self.id
    }

    fn set_id(&mut self, id: Self::Id) {
        self.id = id;
    }
}

impl PartialEq<ProfileListItem> for ProfileId {
    fn eq(&self, item: &ProfileListItem) -> bool {
        self == item.id()
    }
}

impl From<&Profile> for ProfileListItem {
    fn from(profile: &Profile) -> Self {
        Self {
            id: profile.id.clone(),
            name: profile.name().to_owned(),
        }
    }
}

impl Generate for &ProfileListItem {
    type Output<'this>
        = Span<'this>
    where
        Self: 'this;

    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this,
    {
        self.name.as_str().into()
    }
}

/// TODO
#[derive(Debug, Default)]
pub struct ProfilePreview {
    id: ComponentId,
    fields: StateCell<Option<ProfileId>, Vec<(String, TemplatePreview)>>,
}

impl Component for ProfilePreview {
    fn id(&self) -> ComponentId {
        self.id
    }
}

impl<'a> Draw<ProfilePreviewProps<'a>> for ProfilePreview {
    fn draw(
        &self,
        canvas: &mut Canvas,
        props: ProfilePreviewProps<'a>,
        metadata: DrawMetadata,
    ) {
        let title = TuiContext::get()
            .input_engine
            .add_hint("Profile", Action::SelectProfile);
        let block = Pane {
            title: &title,
            has_focus: metadata.has_focus(),
        }
        .generate();
        let area = block.inner(metadata.area());
        canvas.render_widget(block, metadata.area());

        // Whenever the selected profile changes, rebuild the internal state.
        // This is needed because the template preview rendering is async.
        let profile_id = props.profile_id;
        let fields = self.fields.get_or_update(&profile_id.cloned(), || {
            let collection = ViewContext::collection();
            let Some(profile_data) = profile_id.and_then(|profile_id| {
                let profile = collection
                    .profiles
                    .get(profile_id)
                    // Failure is a logic error
                    .ok_or_else(|| anyhow!("No profile with ID `{profile_id}`"))
                    .reported(&ViewContext::messages_tx())?;
                Some(&profile.data)
            }) else {
                return Default::default();
            };
            profile_data
                .iter()
                .map(|(key, template)| {
                    (
                        key.clone(),
                        TemplatePreview::new(
                            template.clone(),
                            None,
                            false,
                            // We don't know how this value will be used, so
                            // let's say we *do* support streaming to prevent
                            // loading some huge streams
                            true,
                        ),
                    )
                })
                .collect_vec()
        });

        let table = Table {
            header: Some(["Field", "Value"]),
            rows: fields
                .iter()
                .map(|(key, value)| [key.as_str().into(), value.generate()])
                .collect_vec(),
            alternate_row_style: true,
            ..Default::default()
        };
        canvas.render_widget(table, area);
    }
}

/// Props for [ProfilePreview]
pub struct ProfilePreviewProps<'a> {
    pub profile_id: Option<&'a ProfileId>,
}
