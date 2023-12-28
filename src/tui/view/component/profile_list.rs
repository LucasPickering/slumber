use crate::{
    collection::{Profile, ProfileId},
    tui::view::{
        common::{list::List, Pane},
        component::primary::PrimaryPane,
        draw::{Draw, Generate},
        event::{Event, EventHandler, UpdateContext},
        state::{
            persistence::{Persistable, Persistent, PersistentKey},
            select::{Dynamic, SelectState},
        },
        Component,
    },
};
use ratatui::{prelude::Rect, Frame};

#[derive(Debug)]
pub struct ProfileListPane {
    profiles: Component<Persistent<SelectState<Dynamic, Profile>>>,
}

pub struct ProfileListPaneProps {
    pub is_selected: bool,
}

impl ProfileListPane {
    pub fn new(profiles: Vec<Profile>) -> Self {
        // Loaded request depends on the profile, so refresh on change
        fn on_select(context: &mut UpdateContext, _: &mut Profile) {
            context.queue_event(Event::HttpLoadRequest);
        }

        Self {
            profiles: Persistent::new(
                PersistentKey::ProfileId,
                SelectState::new(profiles).on_select(on_select),
            )
            .into(),
        }
    }

    pub fn profiles(&self) -> &SelectState<Dynamic, Profile> {
        &self.profiles
    }
}

impl EventHandler for ProfileListPane {
    fn children(&mut self) -> Vec<Component<&mut dyn EventHandler>> {
        vec![self.profiles.as_child()]
    }
}

impl Draw<ProfileListPaneProps> for ProfileListPane {
    fn draw(&self, frame: &mut Frame, props: ProfileListPaneProps, area: Rect) {
        self.profiles.set_area(area); // Needed for tracking cursor events
        let title = PrimaryPane::ProfileList.to_string();
        let list = List {
            block: Some(Pane {
                title: &title,
                is_focused: props.is_selected,
            }),
            list: &self.profiles,
        };
        frame.render_stateful_widget(
            list.generate(),
            area,
            &mut self.profiles.state_mut(),
        )
    }
}

/// Persist profile by ID
impl Persistable for Profile {
    type Persisted = ProfileId;

    fn get_persistent(&self) -> &Self::Persisted {
        &self.id
    }
}
