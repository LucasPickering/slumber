use crate::{
    collection::{Profile, ProfileId},
    tui::{
        context::TuiContext,
        input::Action,
        message::MessageSender,
        view::{
            common::{list::List, Pane},
            draw::{Draw, Generate},
            event::{Event, EventHandler, EventQueue, Update},
            state::{
                persistence::{Persistable, Persistent, PersistentKey},
                select::SelectState,
            },
            Component,
        },
    },
};
use ratatui::{
    prelude::Rect,
    style::{Modifier, Style},
    widgets::Paragraph,
    Frame,
};

#[derive(Debug)]
pub struct ProfileListPane {
    profiles: Component<Persistent<SelectState<Profile>>>,
}

pub struct ProfileListPaneProps {
    pub is_selected: bool,
}

impl ProfileListPane {
    pub fn new(profiles: Vec<Profile>) -> Self {
        // Loaded request depends on the profile, so refresh on change
        fn on_select(_: &mut Profile) {
            EventQueue::push(Event::HttpLoadRequest);
        }

        Self {
            profiles: Persistent::new(
                PersistentKey::ProfileId,
                SelectState::builder(profiles).on_select(on_select).build(),
            )
            .into(),
        }
    }

    pub fn profiles(&self) -> &SelectState<Profile> {
        self.profiles.data()
    }
}

impl EventHandler for ProfileListPane {
    fn update(&mut self, _: &MessageSender, event: Event) -> Update {
        if let Some(Action::Submit) = event.action() {
            // Sending requests from the profile pane is unintuitive, so eat
            // submission events here
            Update::Consumed
        } else {
            Update::Propagate(event)
        }
    }

    fn children(&mut self) -> Vec<Component<&mut dyn EventHandler>> {
        vec![self.profiles.as_child()]
    }
}

impl Draw<ProfileListPaneProps> for ProfileListPane {
    fn draw(&self, frame: &mut Frame, props: ProfileListPaneProps, area: Rect) {
        let title = TuiContext::get()
            .input_engine
            .add_hint("Profiles", Action::SelectProfileList);
        let block = Pane {
            title: &title,
            is_focused: props.is_selected,
        }
        .generate();
        let inner_area = block.inner(area);
        frame.render_widget(block, area);

        if props.is_selected {
            // Only show the full list if selected
            self.profiles.draw(
                frame,
                List {
                    block: None,
                    list: self.profiles.data().items(),
                }
                .generate(),
                inner_area,
            );
        } else {
            // Pane is not selected - just show the selected profile
            let profile = self
                .profiles()
                .selected()
                .map(|profile| profile.name())
                .unwrap_or("<none>");
            frame.render_widget(
                Paragraph::new(profile)
                    .style(Style::new().add_modifier(Modifier::BOLD)),
                inner_area,
            )
        }
    }
}

/// Persist profile by ID
impl Persistable for Profile {
    type Persisted = ProfileId;

    fn get_persistent(&self) -> &Self::Persisted {
        &self.id
    }
}

/// Needed for persistence loading
impl PartialEq<Profile> for ProfileId {
    fn eq(&self, other: &Profile) -> bool {
        self == &other.id
    }
}
