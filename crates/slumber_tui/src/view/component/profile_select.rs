//! Components related to the selection of profiles

use crate::{
    context::TuiContext,
    view::{
        common::{
            list::List, modal::Modal, table::Table,
            template_preview::TemplatePreview, Pane,
        },
        context::PersistedLazy,
        draw::{Draw, DrawMetadata, Generate},
        event::{Event, EventHandler, Update},
        state::{select::SelectState, StateCell},
        Component, ModalPriority, ViewContext,
    },
};
use itertools::Itertools;
use persisted::PersistedKey;
use ratatui::{
    layout::{Constraint, Layout},
    text::{Line, Text},
    Frame,
};
use serde::Serialize;
use slumber_config::Action;
use slumber_core::{
    collection::{Profile, ProfileId},
    util::doc_link,
};

/// Minimal pane to show the current profile, and handle interaction to open the
/// profile list modal
#[derive(Debug)]
pub struct ProfilePane {
    /// Even though we never use SelectState's event handling or selection
    /// logic, this is the best way to store the state. We need to hang onto
    /// the entire list of items so we can pass it down to the modal, and also
    /// store which is selected. Some alternatives I considered:
    ///
    /// - Store a `Vec<Profile>` and `Option<ProfileId>` separately. This is
    ///   basically the same as a SelectState, but requires bespoke logic to
    ///   correctly handling select defaults, and handling when the persisted
    ///   value goes missing (i.e. profile is deleted from the collection). It
    ///   also complicates persistence a lot because of annoying orphan rule
    ///   stuff.
    /// - Share state between this struct and the modal using reference
    ///   passing. This doesn't work because the select state in this struct
    ///   and the modal can't be the same; when selecting a profile in the
    ///   modal, we *don't* want to select it in the outer app until the user
    ///   hits Enter. In addition, the modal has to be moved out into the modal
    ///   queue in order to achieve the correct render and event handling
    ///   ordering, which is incompatible with shared references.
    /// - Share state via `Rc<RefCell<_>>`. This shares the same core problem
    ///   as the previous issue, and also adds a ton of complexity with types
    ///   and whatnot.
    ///
    /// In conclusion, this component and the modal *have* to have separate
    /// state because the selected values shouldn't necessarily be in sync.
    /// That, combined with the need to have 'static state in order to move it
    /// into the modal, means duplicating SelectState and cloning the contents
    /// is the best way to go.
    profiles: PersistedLazy<SelectedProfileKey, SelectState<Profile>>,
}

/// Persisted key for the ID of the selected profile
#[derive(Debug, Serialize, PersistedKey)]
#[persisted(Option<ProfileId>)]
struct SelectedProfileKey;

impl ProfilePane {
    pub fn new(profiles: Vec<Profile>) -> Self {
        let profiles = SelectState::builder(profiles).build();
        Self {
            profiles: PersistedLazy::new(SelectedProfileKey, profiles),
        }
    }

    pub fn selected_profile(&self) -> Option<&Profile> {
        self.profiles.selected()
    }

    /// Open the profile list modal
    pub fn open_modal(&self) {
        ViewContext::open_modal(
            ProfileListModal::new(
                // See self.profiles doc comment for why we need to clone
                self.profiles
                    .items()
                    .iter()
                    .map(|item| item.value.clone())
                    .collect(),
                self.profiles.selected().map(|profile| &profile.id),
            ),
            ModalPriority::Low,
        );
    }
}

impl EventHandler for ProfilePane {
    fn update(&mut self, event: Event) -> Update {
        if let Some(Action::LeftClick) = event.action() {
            self.open_modal();
        } else if let Some(SelectProfile(profile_id)) = event.local() {
            // Handle message from the modal
            self.profiles.select(profile_id);
            ViewContext::push_event(Event::HttpSelectRequest(None));
        } else {
            return Update::Propagate(event);
        }
        Update::Consumed
    }
}

impl Draw for ProfilePane {
    fn draw(&self, frame: &mut Frame, _: (), metadata: DrawMetadata) {
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

        frame.render_widget(
            if let Some(profile) = self.selected_profile() {
                profile.name()
            } else {
                "No profiles defined"
            },
            area,
        );
    }
}

/// Local event to pass selected profile ID from modal back to the parent
#[derive(Debug)]
struct SelectProfile(ProfileId);

/// Modal to allow user to select a profile from a list and preview profile
/// fields
#[derive(Debug)]
pub struct ProfileListModal {
    select: Component<SelectState<Profile>>,
    detail: Component<ProfileDetail>,
}

impl ProfileListModal {
    pub fn new(
        profiles: Vec<Profile>,
        selected_profile: Option<&ProfileId>,
    ) -> Self {
        // Loaded request depends on the profile, so refresh on change
        fn on_submit(profile: &mut Profile) {
            // Close the modal *first*, so the parent can handle the
            // callback event. Jank but it works
            ViewContext::push_event(Event::CloseModal);
            ViewContext::push_event(Event::new_local(SelectProfile(
                profile.id.clone(),
            )));
        }

        let select = SelectState::builder(profiles)
            .preselect_opt(selected_profile)
            .on_submit(on_submit)
            .build();
        Self {
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
    fn children(&mut self) -> Vec<Component<&mut dyn EventHandler>> {
        vec![self.select.as_child()]
    }
}

impl Draw for ProfileListModal {
    fn draw(&self, frame: &mut Frame, _: (), metadata: DrawMetadata) {
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
                ProfileDetailProps { profile },
                detail_area,
                false,
            )
        }
    }
}

/// Display the contents of a profile
#[derive(Debug, Default)]
pub struct ProfileDetail {
    fields: StateCell<ProfileId, Vec<(String, TemplatePreview)>>,
}

pub struct ProfileDetailProps<'a> {
    pub profile: &'a Profile,
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
        let fields =
            self.fields.get_or_update(props.profile.id.clone(), || {
                props
                    .profile
                    .data
                    .iter()
                    .map(|(key, template)| {
                        (
                            key.clone(),
                            TemplatePreview::new(
                                template.clone(),
                                Some(props.profile.id.clone()),
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
