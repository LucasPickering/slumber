//! Components related to the selection of profiles

use crate::{
    context::TuiContext,
    util::ResultReported,
    view::{
        Component, ViewContext,
        common::{
            Pane,
            list::List,
            modal::{Modal, ModalHandle},
            table::Table,
            template_preview::TemplatePreview,
        },
        context::UpdateContext,
        draw::{Draw, DrawMetadata, Generate},
        event::{Child, Emitter, Event, EventHandler, OptionEvent, ToEmitter},
        state::{
            StateCell,
            select::{SelectState, SelectStateEvent, SelectStateEventType},
        },
        util::persistence::Persisted,
    },
};
use anyhow::anyhow;
use itertools::Itertools;
use persisted::PersistedKey;
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    text::{Line, Text},
};
use serde::Serialize;
use slumber_config::Action;
use slumber_core::collection::{Collection, HasId, Profile, ProfileId};
use slumber_util::doc_link;

/// Minimal pane to show the current profile, and handle interaction to open the
/// profile list modal
#[derive(Debug)]
pub struct ProfilePane {
    /// Store just the ID of the selected profile. We'll load the full list
    /// from the view context when opening the modal. It's not possible to
    /// share selection state with the modal, because the two values aren't
    /// necessarily the same: the user could highlight a profile without
    /// actually selecting it.
    selected_profile_id: Persisted<SelectedProfileKey>,
    /// Handle events from the opened modal
    modal_handle: ModalHandle<SelectProfile>,
}

/// Persisted key for the ID of the selected profile
#[derive(Debug, Serialize, PersistedKey)]
#[persisted(Option<ProfileId>)]
struct SelectedProfileKey;

impl ProfilePane {
    pub fn new(collection: &Collection) -> Self {
        let mut selected_profile_id =
            Persisted::new_default(SelectedProfileKey);

        // Two invalid cases we need to handle here:
        // - Nothing is persisted but the map has values now
        // - Persisted ID isn't in the map now
        // In either case, just fall back to:
        // - Default profile if available
        // - First profile if available
        // - `None` if map is empty
        match &*selected_profile_id {
            Some(id) if collection.profiles.contains_key(id) => {}
            _ => {
                *selected_profile_id.get_mut() = collection
                    .default_profile()
                    .or(collection.profiles.values().next())
                    .map(Profile::id)
                    .cloned();
            }
        }

        Self {
            selected_profile_id,
            modal_handle: ModalHandle::new(),
        }
    }

    pub fn selected_profile_id(&self) -> Option<&ProfileId> {
        self.selected_profile_id.as_ref()
    }

    /// Open the profile list modal
    pub fn open_modal(&mut self) {
        self.modal_handle
            .open(ProfileListModal::new(self.selected_profile_id.as_ref()));
    }
}

impl EventHandler for ProfilePane {
    fn update(&mut self, _: &mut UpdateContext, event: Event) -> Option<Event> {
        event
            .opt()
            .action(|action, propagate| match action {
                Action::LeftClick => self.open_modal(),
                _ => propagate.set(),
            })
            .emitted(
                self.modal_handle.to_emitter(),
                |SelectProfile(profile_id)| {
                    // Handle message from the modal
                    *self.selected_profile_id.get_mut() =
                        Some(profile_id.clone());
                    // Refresh template previews
                    ViewContext::push_event(Event::HttpSelectRequest(None));
                },
            )
    }
}

impl Draw for ProfilePane {
    fn draw(&self, frame: &mut Frame, (): (), metadata: DrawMetadata) {
        let title = TuiContext::get()
            .input_engine
            .add_hint("Profile", Action::SelectProfileList);
        let block = Pane {
            title: &title,
            has_focus: false,
        }
        .generate();
        frame.render_widget(&block, metadata.area());
        let area = block.inner(metadata.area());

        // Grab global profile selection state
        let collection = ViewContext::collection();
        let selected_profile = (*self.selected_profile_id)
            .as_ref()
            .and_then(|profile_id| collection.profiles.get(profile_id));
        frame.render_widget(
            if let Some(profile) = selected_profile {
                profile.name()
            } else {
                "No profiles defined"
            },
            area,
        );
    }
}

/// Modal to allow user to select a profile from a list and preview profile
/// fields
#[derive(Debug)]
struct ProfileListModal {
    emitter: Emitter<SelectProfile>,
    select: Component<SelectState<ProfileListItem>>,
    detail: Component<ProfileDetail>,
}

impl ProfileListModal {
    pub fn new(selected_profile_id: Option<&ProfileId>) -> Self {
        let profiles = ViewContext::collection()
            .profiles
            .values()
            .map(ProfileListItem::from)
            .collect();

        let select = SelectState::builder(profiles)
            .preselect_opt(selected_profile_id)
            .subscribe([SelectStateEventType::Submit])
            .build();
        Self {
            emitter: Default::default(),
            select: select.into(),
            detail: Default::default(),
        }
    }
}

impl Modal for ProfileListModal {
    fn title(&self) -> Line<'_> {
        "Profiles".into()
    }

    fn dimensions(&self) -> (Constraint, Constraint) {
        (Constraint::Percentage(60), Constraint::Percentage(40))
    }
}

impl EventHandler for ProfileListModal {
    fn update(&mut self, _: &mut UpdateContext, event: Event) -> Option<Event> {
        event.opt().emitted(self.select.to_emitter(), |event| {
            // Loaded request depends on the profile, so refresh on change
            if let SelectStateEvent::Submit(index) = event {
                // Close modal first so the parent can consume the emitted event
                self.close(true);
                let profile_id = self.select.data()[index].id.clone();
                self.emitter.emit(SelectProfile(profile_id));
            }
        })
    }

    fn children(&mut self) -> Vec<Component<Child<'_>>> {
        vec![self.select.to_child_mut()]
    }
}

impl Draw for ProfileListModal {
    fn draw(&self, frame: &mut Frame, (): (), metadata: DrawMetadata) {
        // Empty state
        let select = self.select.data();
        if select.is_empty() {
            frame.render_widget(
                Text::from(vec![
                    "No profiles defined; add one to your collection.".into(),
                    doc_link("api/request_collection/profile").into(),
                ]),
                metadata.area(),
            );
            return;
        }

        let [list_area, _, detail_area] = Layout::vertical([
            Constraint::Length(select.len().min(5) as u16),
            Constraint::Length(1), // Padding
            Constraint::Min(0),
        ])
        .areas(metadata.area());

        self.select.draw(frame, List::from(select), list_area, true);
        if let Some(profile) = select.selected() {
            self.detail.draw(
                frame,
                ProfileDetailProps {
                    profile_id: &profile.id,
                },
                detail_area,
                false,
            );
        }
    }
}

impl ToEmitter<SelectProfile> for ProfileListModal {
    fn to_emitter(&self) -> Emitter<SelectProfile> {
        self.emitter
    }
}

/// Local event to pass selected profile ID from modal back to the parent
#[derive(Debug)]
struct SelectProfile(ProfileId);

/// Simplified version of [Profile], to be used in the display list. This
/// only stores whatever data is necessary to render the list
#[derive(Clone, Debug)]
struct ProfileListItem {
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
        = Text<'this>
    where
        Self: 'this;

    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this,
    {
        self.name.as_str().into()
    }
}

/// Display the contents of a profile
#[derive(Debug, Default)]
struct ProfileDetail {
    fields: StateCell<ProfileId, Vec<(String, TemplatePreview)>>,
}

struct ProfileDetailProps<'a> {
    profile_id: &'a ProfileId,
}

impl<'a> Draw<ProfileDetailProps<'a>> for ProfileDetail {
    fn draw(
        &self,
        frame: &mut Frame,
        props: ProfileDetailProps<'a>,
        metadata: DrawMetadata,
    ) {
        // Whenever the selected profile changes, rebuild the internal state.
        // This is needed because the template preview rendering is async.
        let profile_id = props.profile_id;
        let fields = self.fields.get_or_update(profile_id, || {
            let collection = ViewContext::collection();
            let Some(profile_data) = collection
                .profiles
                .get(profile_id)
                // Failure is a logic error
                .ok_or_else(|| anyhow!("No profile with ID `{profile_id}`"))
                .reported(&ViewContext::messages_tx())
                .map(|profile| &profile.data)
            else {
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
        frame.render_widget(table.generate(), metadata.area());
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        test_util::{TestHarness, harness},
        view::util::persistence::DatabasePersistedStore,
    };
    use persisted::PersistedStore;
    use rstest::rstest;
    use slumber_core::test_util::by_id;
    use slumber_util::Factory;

    use super::*;

    /// Test various scenarios when loading the selected profile ID from
    /// persistence
    #[rstest]
    #[case::empty(&[] , None, None)]
    #[case::preselect(&["p1", "p2", "default"] , None, Some("default"))]
    #[case::unknown(&["p1", "p2", "default"] , Some("p3"), Some("default"))]
    #[case::unknown_empty(&[] , Some("p1"), None)]
    #[case::persisted(&["p1", "p2", "default"] , Some("p2"), Some("p2"))]
    fn test_initial_profile(
        _harness: TestHarness,
        #[case] profile_ids: &[&str],
        #[case] persisted_id: Option<&str>,
        #[case] expected: Option<&str>,
    ) {
        let profiles = by_id(profile_ids.iter().map(|&id| Profile {
            id: id.into(),
            default: id == "default",
            ..Profile::factory(())
        }));
        if let Some(persisted_id) = persisted_id {
            DatabasePersistedStore::store_persisted(
                &SelectedProfileKey,
                &Some(persisted_id.into()),
            );
        }

        let expected = expected.map(ProfileId::from);
        let component = ProfilePane::new(&Collection {
            profiles,
            ..Collection::factory(())
        });
        assert_eq!(*component.selected_profile_id, expected);
    }
}
