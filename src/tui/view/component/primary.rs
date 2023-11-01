//! Components for the "primary" view, which is the paned request/response view

use crate::{
    config::{Profile, RequestCollection, RequestRecipe},
    tui::{
        input::Action,
        message::Message,
        view::{
            component::{
                request::{RequestPane, RequestPaneProps},
                response::{ResponsePane, ResponsePaneProps},
                Component, Draw, Event, UpdateContext, UpdateOutcome,
            },
            state::{FixedSelect, RequestState, StatefulList, StatefulSelect},
            util::{layout, BlockBrick, ListBrick, ToTui},
            Frame, RenderContext,
        },
    },
};
use derive_more::Display;
use ratatui::prelude::{Constraint, Direction, Rect};
use strum::EnumIter;

/// Primary TUI view, which shows request/response panes
#[derive(Debug, Display)]
#[display(fmt = "PrimaryView")]
pub struct PrimaryView {
    // Own state
    selected_pane: StatefulSelect<PrimaryPane>,

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
#[derive(Copy, Clone, Debug, derive_more::Display, EnumIter, PartialEq)]
pub enum PrimaryPane {
    #[display(fmt = "Profiles")]
    ProfileList,
    #[display(fmt = "Recipes")]
    RecipeList,
    Request,
    Response,
}

impl FixedSelect for PrimaryPane {
    const DEFAULT_INDEX: usize = 1;
}

impl PrimaryView {
    pub fn new(collection: &RequestCollection) -> Self {
        Self {
            selected_pane: StatefulSelect::default(),

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

    /// Expose response pane, for fullscreening
    pub fn response_pane(&self) -> &ResponsePane {
        &self.response_pane
    }

    /// Expose response pane, for fullscreening
    pub fn response_pane_mut(&mut self) -> &mut ResponsePane {
        &mut self.response_pane
    }
}

impl Component for PrimaryView {
    fn update(
        &mut self,
        context: &mut UpdateContext,
        message: Event,
    ) -> UpdateOutcome {
        match message {
            // Send HTTP request (bubbled up from child)
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
                UpdateOutcome::Consumed
            }

            // Input messages
            Event::Input {
                action: Some(Action::FocusPrevious),
                ..
            } => {
                self.selected_pane.previous();
                UpdateOutcome::Consumed
            }
            Event::Input {
                action: Some(Action::FocusNext),
                ..
            } => {
                self.selected_pane.next();
                UpdateOutcome::Consumed
            }

            _ => UpdateOutcome::Propagate(message),
        }
    }

    fn children(&mut self) -> Vec<&mut dyn Component> {
        vec![match self.selected_pane.selected() {
            PrimaryPane::ProfileList => {
                &mut self.profile_list_pane as &mut dyn Component
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
        context: &RenderContext,
        props: PrimaryViewProps<'a>,
        frame: &mut Frame,
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
            [Constraint::Max(16), Constraint::Min(0)],
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
            frame,
            profiles_chunk,
        );
        self.recipe_list_pane.draw(
            context,
            ListPaneProps {
                is_selected: panes.is_selected(&PrimaryPane::RecipeList),
            },
            frame,
            recipes_chunk,
        );
        self.request_pane.draw(
            context,
            RequestPaneProps {
                is_selected: panes.is_selected(&PrimaryPane::Request),
                selected_recipe: self.selected_recipe(),
            },
            frame,
            request_chunk,
        );
        self.response_pane.draw(
            context,
            ResponsePaneProps {
                is_selected: panes.is_selected(&PrimaryPane::Response),
                active_request: props.active_request,
            },
            frame,
            response_chunk,
        );
    }
}

#[derive(Debug, Display)]
#[display(fmt = "ProfileListPane")]
struct ProfileListPane {
    profiles: StatefulList<Profile>,
}

struct ListPaneProps {
    is_selected: bool,
}

impl ProfileListPane {
    pub fn new(profiles: Vec<Profile>) -> Self {
        Self {
            profiles: StatefulList::with_items(profiles),
        }
    }
}

impl Component for ProfileListPane {
    fn update(
        &mut self,
        _context: &mut UpdateContext,
        message: Event,
    ) -> UpdateOutcome {
        match message {
            Event::Input {
                action: Some(Action::Up),
                ..
            } => {
                self.profiles.previous();
                UpdateOutcome::Consumed
            }
            Event::Input {
                action: Some(Action::Down),
                ..
            } => {
                self.profiles.next();
                UpdateOutcome::Consumed
            }
            _ => UpdateOutcome::Propagate(message),
        }
    }
}

impl Draw<ListPaneProps> for ProfileListPane {
    fn draw(
        &self,
        context: &RenderContext,
        props: ListPaneProps,
        frame: &mut Frame,
        chunk: Rect,
    ) {
        let list = ListBrick {
            block: BlockBrick {
                title: PrimaryPane::ProfileList.to_string(),
                is_focused: props.is_selected,
            },
            list: &self.profiles,
        };
        frame.render_stateful_widget(
            list.to_tui(context),
            chunk,
            &mut self.profiles.state_mut(),
        )
    }
}

#[derive(Debug, Display)]
#[display(fmt = "RecipeListPane")]
struct RecipeListPane {
    recipes: StatefulList<RequestRecipe>,
}

impl RecipeListPane {
    pub fn new(recipes: Vec<RequestRecipe>) -> Self {
        Self {
            recipes: StatefulList::with_items(recipes),
        }
    }
}

impl Component for RecipeListPane {
    fn update(
        &mut self,
        context: &mut UpdateContext,
        message: Event,
    ) -> UpdateOutcome {
        let mut load_from_repo = |pane: &RecipeListPane| -> UpdateOutcome {
            if let Some(recipe) = pane.recipes.selected() {
                context.send_message(Message::RepositoryStartLoad {
                    recipe_id: recipe.id.clone(),
                });
            }
            UpdateOutcome::Consumed
        };

        match message {
            Event::Input {
                action: Some(Action::Interact),
                ..
            } => {
                // Parent has to be responsible for sending the request because
                // it also needs access to the profile list state
                UpdateOutcome::Propagate(Event::HttpSendRequest)
            }
            Event::Input {
                action: Some(Action::Up),
                ..
            } => {
                self.recipes.previous();
                load_from_repo(self)
            }
            Event::Input {
                action: Some(Action::Down),
                ..
            } => {
                self.recipes.next();
                load_from_repo(self)
            }
            _ => UpdateOutcome::Propagate(message),
        }
    }
}

impl Draw<ListPaneProps> for RecipeListPane {
    fn draw(
        &self,
        context: &RenderContext,
        props: ListPaneProps,
        frame: &mut Frame,
        chunk: Rect,
    ) {
        let pane_kind = PrimaryPane::RecipeList;
        let list = ListBrick {
            block: BlockBrick {
                title: pane_kind.to_string(),
                is_focused: props.is_selected,
            },
            list: &self.recipes,
        };
        frame.render_stateful_widget(
            list.to_tui(context),
            chunk,
            &mut self.recipes.state_mut(),
        )
    }
}
