//! Components for the "primary" view, which is the paned request/response view

use crate::{
    collection::{Profile, RequestCollection, RequestRecipe},
    tui::{
        input::Action,
        message::Message,
        view::{
            common::{list::List, Block},
            component::{
                help::HelpModal,
                request::{RequestPane, RequestPaneProps},
                response::{ResponsePane, ResponsePaneProps},
                settings::SettingsModal,
            },
            draw::{Draw, DrawContext, Generate},
            event::{Event, EventHandler, Update, UpdateContext},
            state::{
                select::{Dynamic, Fixed, SelectState},
                RequestState,
            },
            util::layout,
            ModalPriority,
        },
    },
};
use derive_more::Display;
use ratatui::{
    prelude::{Constraint, Direction, Rect},
    widgets::ListState,
};
use strum::EnumIter;

/// Primary TUI view, which shows request/response panes
#[derive(Debug)]
pub struct PrimaryView {
    // Own state
    selected_pane: SelectState<Fixed, PrimaryPane, ListState>,

    // Children
    profile_list_pane: ProfileListPane,
    recipe_list_pane: RecipeListPane,
    request_pane: RequestPane,
    response_pane: ResponsePane,
}

pub struct PrimaryViewProps<'a> {
    pub active_request: Option<&'a RequestState>,
}

/// Selectable panes in the primary view mode
#[derive(Copy, Clone, Debug, Default, Display, EnumIter, PartialEq)]
pub enum PrimaryPane {
    #[display("Profiles")]
    ProfileList,
    #[default]
    #[display("Recipes")]
    RecipeList,
    Request,
    Response,
}

impl PrimaryView {
    pub fn new(collection: &RequestCollection) -> Self {
        Self {
            selected_pane: Default::default(),

            profile_list_pane: ProfileListPane::new(
                collection.profiles.to_owned(),
            ),
            recipe_list_pane: RecipeListPane::new(
                collection.recipes.to_owned(),
            ),
            request_pane: RequestPane::default(),
            response_pane: ResponsePane::default(),
        }
    }

    /// Which recipe in the recipe list is selected? `None` iff the list is
    /// empty.
    pub fn selected_recipe(&self) -> Option<&RequestRecipe> {
        self.recipe_list_pane.recipes.selected()
    }

    /// Which profile in the list is selected? `None` iff the list is empty.
    /// Exposing inner state is hacky but it's an easy shortcut
    pub fn selected_profile(&self) -> Option<&Profile> {
        self.profile_list_pane.profiles.selected()
    }

    /// Expose request pane, for fullscreening
    pub fn request_pane(&self) -> &RequestPane {
        &self.request_pane
    }

    /// Expose request pane, for fullscreening
    pub fn request_pane_mut(&mut self) -> &mut RequestPane {
        &mut self.request_pane
    }

    /// Expose response pane, for fullscreening
    pub fn response_pane(&self) -> &ResponsePane {
        &self.response_pane
    }

    /// Expose response pane, for fullscreening
    pub fn response_pane_mut(&mut self) -> &mut ResponsePane {
        &mut self.response_pane
    }
}

impl EventHandler for PrimaryView {
    fn update(&mut self, context: &mut UpdateContext, event: Event) -> Update {
        match event {
            // Send HTTP request (bubbled up from child *or* queued by parent)
            Event::HttpSendRequest => {
                if let Some(recipe) = self.selected_recipe() {
                    context.send_message(Message::HttpBeginRequest {
                        // Reach into the children to grab state (ugly!)
                        recipe_id: recipe.id.clone(),
                        profile_id: self
                            .selected_profile()
                            .map(|profile| profile.id.clone()),
                    });
                }
                Update::Consumed
            }

            // Input messages
            Event::Input { action, .. } => match action {
                Some(Action::PreviousPane) => {
                    self.selected_pane.previous(context);
                    Update::Consumed
                }
                Some(Action::NextPane) => {
                    self.selected_pane.next(context);
                    Update::Consumed
                }
                Some(Action::SendRequest) => {
                    // Send a request from anywhere
                    context.queue_event(Event::HttpSendRequest);
                    Update::Consumed
                }
                Some(Action::OpenSettings) => {
                    context.queue_event(Event::OpenModal {
                        modal: Box::<SettingsModal>::default(),
                        priority: ModalPriority::Low,
                    });
                    Update::Consumed
                }
                Some(Action::OpenHelp) => {
                    context.queue_event(Event::OpenModal {
                        modal: Box::<HelpModal>::default(),
                        priority: ModalPriority::Low,
                    });
                    Update::Consumed
                }
                _ => Update::Propagate(event),
            },

            _ => Update::Propagate(event),
        }
    }

    fn children(&mut self) -> Vec<&mut dyn EventHandler> {
        vec![match self.selected_pane.selected() {
            PrimaryPane::ProfileList => {
                &mut self.profile_list_pane as &mut dyn EventHandler
            }
            PrimaryPane::RecipeList => &mut self.recipe_list_pane,
            PrimaryPane::Request => &mut self.request_pane,
            PrimaryPane::Response => &mut self.response_pane,
        }]
    }
}

impl<'a> Draw<PrimaryViewProps<'a>> for PrimaryView {
    fn draw(
        &self,
        context: &mut DrawContext,
        props: PrimaryViewProps<'a>,
        chunk: Rect,
    ) {
        // Split the main pane horizontally
        let [left_chunk, right_chunk] = layout(
            chunk,
            Direction::Horizontal,
            [Constraint::Max(40), Constraint::Percentage(50)],
        );

        // Split left column vertically
        let [profiles_chunk, recipes_chunk] = layout(
            left_chunk,
            Direction::Vertical,
            [
                // Make profile list as small as possible, with a max size
                Constraint::Max(
                    self.profile_list_pane.profiles.len().clamp(1, 16) as u16
                        + 2, // Account for top/bottom border
                ),
                Constraint::Min(0),
            ],
        );

        // Split right column vertically
        let [request_chunk, response_chunk] = layout(
            right_chunk,
            Direction::Vertical,
            [Constraint::Percentage(50), Constraint::Percentage(50)],
        );

        // Primary panes
        let panes = &self.selected_pane;
        self.profile_list_pane.draw(
            context,
            ListPaneProps {
                is_selected: panes.is_selected(&PrimaryPane::ProfileList),
            },
            profiles_chunk,
        );
        self.recipe_list_pane.draw(
            context,
            ListPaneProps {
                is_selected: panes.is_selected(&PrimaryPane::RecipeList),
            },
            recipes_chunk,
        );
        self.request_pane.draw(
            context,
            RequestPaneProps {
                is_selected: panes.is_selected(&PrimaryPane::Request),
                selected_recipe: self.selected_recipe(),
                selected_profile_id: self
                    .selected_profile()
                    .map(|profile| &profile.id),
            },
            request_chunk,
        );
        self.response_pane.draw(
            context,
            ResponsePaneProps {
                is_selected: panes.is_selected(&PrimaryPane::Response),
                active_request: props.active_request,
            },
            response_chunk,
        );
    }
}

#[derive(Debug)]
struct ProfileListPane {
    profiles: SelectState<Dynamic, Profile>,
}

struct ListPaneProps {
    is_selected: bool,
}

impl ProfileListPane {
    pub fn new(profiles: Vec<Profile>) -> Self {
        Self {
            profiles: SelectState::new(profiles),
        }
    }
}

impl EventHandler for ProfileListPane {
    fn children(&mut self) -> Vec<&mut dyn EventHandler> {
        vec![&mut self.profiles]
    }
}

impl Draw<ListPaneProps> for ProfileListPane {
    fn draw(
        &self,
        context: &mut DrawContext,
        props: ListPaneProps,
        chunk: Rect,
    ) {
        let list = List {
            block: Block {
                title: &PrimaryPane::ProfileList.to_string(),
                is_focused: props.is_selected,
            },
            list: &self.profiles,
        };
        context.frame.render_stateful_widget(
            list.generate(),
            chunk,
            &mut self.profiles.state_mut(),
        )
    }
}

#[derive(Debug)]
struct RecipeListPane {
    recipes: SelectState<Dynamic, RequestRecipe>,
}

impl RecipeListPane {
    pub fn new(recipes: Vec<RequestRecipe>) -> Self {
        // When highlighting a new recipe, load it from the repo
        let on_select = |context: &mut UpdateContext,
                         recipe: &RequestRecipe| {
            context.send_message(Message::RepositoryStartLoad {
                recipe_id: recipe.id.clone(),
            });
        };

        // Trigger a request on submit
        let on_submit = |context: &mut UpdateContext, _: &RequestRecipe| {
            // Parent has to be responsible for actually sending the request
            // because it also needs access to the profile list state
            context.queue_event(Event::HttpSendRequest);
        };

        Self {
            recipes: SelectState::new(recipes)
                .on_select(on_select)
                .on_submit(on_submit),
        }
    }
}

impl EventHandler for RecipeListPane {
    fn children(&mut self) -> Vec<&mut dyn EventHandler> {
        vec![&mut self.recipes]
    }
}

impl Draw<ListPaneProps> for RecipeListPane {
    fn draw(
        &self,
        context: &mut DrawContext,
        props: ListPaneProps,
        chunk: Rect,
    ) {
        let pane_kind = PrimaryPane::RecipeList;
        let list = List {
            block: Block {
                title: &pane_kind.to_string(),
                is_focused: props.is_selected,
            },
            list: &self.recipes,
        };
        context.frame.render_stateful_widget(
            list.generate(),
            chunk,
            &mut self.recipes.state_mut(),
        )
    }
}
