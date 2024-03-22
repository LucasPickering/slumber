//! Components for the "primary" view, which is the paned request/response view

use crate::{
    collection::{Collection, Profile, Recipe},
    tui::{
        context::TuiContext,
        input::Action,
        message::{Message, RequestConfig},
        view::{
            common::actions::ActionsModal,
            component::{
                help::HelpModal,
                profile::{ProfilePane, ProfilePaneProps},
                profile_list::{ProfileListPane, ProfileListPaneProps},
                recipe::{RecipePane, RecipePaneProps},
                recipe_list::{RecipeListPane, RecipeListPaneProps},
                response::{ResponsePane, ResponsePaneProps},
            },
            draw::Draw,
            event::{Event, EventHandler, Update, UpdateContext},
            state::{
                persistence::{Persistent, PersistentKey},
                select::{Fixed, SelectState},
                RequestState,
            },
            util::layout,
            Component,
        },
    },
};
use derive_more::Display;
use itertools::Itertools;
use ratatui::{
    prelude::{Constraint, Direction, Rect},
    Frame,
};
use serde::{Deserialize, Serialize};
use strum::{EnumCount, EnumIter};

/// Primary TUI view, which shows request/response panes
#[derive(derive_more::Debug)]
pub struct PrimaryView {
    // Own state
    selected_pane: Persistent<SelectState<Fixed, PrimaryPane>>,
    fullscreen_mode: Persistent<Option<FullscreenMode>>,

    // Children
    #[debug(skip)]
    profile_list_pane: Component<ProfileListPane>,
    #[debug(skip)]
    recipe_list_pane: Component<RecipeListPane>,
    #[debug(skip)]
    profile_pane: Component<ProfilePane>,
    #[debug(skip)]
    request_pane: Component<RecipePane>,
    #[debug(skip)]
    response_pane: Component<ResponsePane>,
}

pub struct PrimaryViewProps<'a> {
    pub active_request: Option<&'a RequestState>,
}

/// Selectable panes in the primary view mode
#[derive(
    Copy,
    Clone,
    Debug,
    Display,
    EnumCount,
    EnumIter,
    PartialEq,
    Serialize,
    Deserialize,
)]
pub enum PrimaryPane {
    ProfileList,
    RecipeList,
    Recipe,
    Response,
}

/// The various things that can be requested (haha get it, requested) to be
/// shown in fullscreen. If one of these is requested while not available, we
/// simply won't show it.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
enum FullscreenMode {
    /// Fullscreen the active request recipe
    Request,
    /// Fullscreen the active response
    Response,
}

impl PrimaryView {
    pub fn new(collection: &Collection) -> Self {
        let profile_list_pane = ProfileListPane::new(
            collection.profiles.values().cloned().collect_vec(),
        )
        .into();
        let recipe_list_pane = RecipeListPane::new(
            collection.recipes.values().cloned().collect_vec(),
        )
        .into();
        Self {
            selected_pane: Persistent::new(
                PersistentKey::PrimaryPane,
                Default::default(),
            ),
            fullscreen_mode: Persistent::new(
                PersistentKey::FullscreenMode,
                None,
            ),

            profile_list_pane,
            recipe_list_pane,
            profile_pane: Default::default(),
            request_pane: Default::default(),
            response_pane: Default::default(),
        }
    }

    /// Which recipe in the recipe list is selected? `None` iff the list is
    /// empty.
    pub fn selected_recipe(&self) -> Option<&Recipe> {
        self.recipe_list_pane.recipes().selected()
    }

    /// Which profile in the list is selected? `None` iff the list is empty.
    /// Exposing inner state is hacky but it's an easy shortcut
    pub fn selected_profile(&self) -> Option<&Profile> {
        self.profile_list_pane.profiles().selected()
    }

    fn toggle_fullscreen(&mut self, mode: FullscreenMode) {
        // If we're already in the given mode, exit
        *self.fullscreen_mode = if Some(mode) == *self.fullscreen_mode {
            None
        } else {
            Some(mode)
        };
    }

    /// Draw the "normal" view, when nothing is full
    fn draw_all_panes(
        &self,
        frame: &mut Frame,
        props: PrimaryViewProps,
        area: Rect,
    ) {
        // Split the main pane horizontally
        let [left_area, right_area] = layout(
            area,
            Direction::Horizontal,
            [Constraint::Max(40), Constraint::Min(40)],
        );

        // Split left column vertically
        let [profiles_area, recipes_area] = layout(
            left_area,
            Direction::Vertical,
            [
                // Minimize pane if not selected
                if self.selected_pane.is_selected(&PrimaryPane::ProfileList) {
                    // Make profile list as small as possible
                    Constraint::Max(
                        // +2 to account for top/bottom border
                        self.profile_list_pane.profiles().len() as u16 + 2,
                    )
                } else {
                    Constraint::Max(3)
                },
                Constraint::Min(0),
            ],
        );

        // Split right column vertically
        let [request_area, response_area] = layout(
            right_area,
            Direction::Vertical,
            [Constraint::Ratio(1, 3), Constraint::Ratio(2, 3)],
        );

        // Primary panes
        let panes = &self.selected_pane;
        self.profile_list_pane.draw(
            frame,
            ProfileListPaneProps {
                is_selected: panes.is_selected(&PrimaryPane::ProfileList),
            },
            profiles_area,
        );
        self.recipe_list_pane.draw(
            frame,
            RecipeListPaneProps {
                is_selected: panes.is_selected(&PrimaryPane::RecipeList),
            },
            recipes_area,
        );

        // If profile list is selected, show the profile contents.
        // Otherwise show the recipe pane
        if let (PrimaryPane::ProfileList, Some(profile)) =
            (self.selected_pane.selected(), self.selected_profile())
        {
            self.profile_pane.draw(
                frame,
                ProfilePaneProps { profile },
                request_area,
            )
        } else {
            self.request_pane.draw(
                frame,
                RecipePaneProps {
                    is_selected: panes.is_selected(&PrimaryPane::Recipe),
                    selected_recipe: self.selected_recipe(),
                    selected_profile_id: self
                        .selected_profile()
                        .map(|profile| &profile.id),
                },
                request_area,
            );
        }

        self.response_pane.draw(
            frame,
            ResponsePaneProps {
                is_selected: panes.is_selected(&PrimaryPane::Response),
                active_request: props.active_request,
            },
            response_area,
        );
    }
}

impl EventHandler for PrimaryView {
    fn update(&mut self, context: &mut UpdateContext, event: Event) -> Update {
        match &event {
            // Load latest request for selected recipe from database
            Event::HttpLoadRequest => {
                if let Some(recipe) = self.selected_recipe() {
                    TuiContext::send_message(Message::RequestLoad {
                        profile_id: self
                            .selected_profile()
                            .map(|profile| profile.id.clone()),
                        recipe_id: recipe.id.clone(),
                    });
                }
            }
            // Send HTTP request
            Event::HttpSendRequest => {
                if let Some(recipe) = self.selected_recipe() {
                    TuiContext::send_message(Message::HttpBeginRequest(
                        RequestConfig {
                            // Reach into the children to grab state (ugly!)
                            recipe_id: recipe.id.clone(),
                            profile_id: self
                                .selected_profile()
                                .map(|profile| profile.id.clone()),
                            options: self.request_pane.recipe_options(),
                        },
                    ));
                }
            }

            // Input messages
            Event::Input {
                action: Some(action),
                event: term_event,
            } => match action {
                Action::LeftClick => {
                    let crossterm::event::Event::Mouse(mouse) = term_event
                    else {
                        unreachable!("Mouse action must have mouse event")
                    };
                    // See if any child panes were clicked
                    if self.profile_list_pane.intersects(mouse) {
                        self.selected_pane
                            .select(context, &PrimaryPane::ProfileList);
                    } else if self.recipe_list_pane.intersects(mouse) {
                        self.selected_pane
                            .select(context, &PrimaryPane::RecipeList);
                    } else if self.request_pane.intersects(mouse) {
                        self.selected_pane
                            .select(context, &PrimaryPane::Recipe);
                    } else if self.response_pane.intersects(mouse) {
                        self.selected_pane
                            .select(context, &PrimaryPane::Response);
                    }
                }
                Action::PreviousPane if self.fullscreen_mode.is_none() => {
                    self.selected_pane.previous(context);
                }
                Action::NextPane if self.fullscreen_mode.is_none() => {
                    self.selected_pane.next(context);
                }
                Action::SendRequest => {
                    // Send a request from anywhere
                    context.queue_event(Event::HttpSendRequest);
                }
                Action::OpenActions => {
                    context.open_modal_default::<ActionsModal>();
                }
                Action::OpenHelp => {
                    context.open_modal_default::<HelpModal>();
                }
                Action::SelectProfileList => self
                    .selected_pane
                    .select(context, &PrimaryPane::ProfileList),
                Action::SelectRecipeList => {
                    self.selected_pane.select(context, &PrimaryPane::RecipeList)
                }
                Action::SelectRecipe => {
                    self.selected_pane.select(context, &PrimaryPane::Recipe)
                }
                Action::SelectResponse => {
                    self.selected_pane.select(context, &PrimaryPane::Response)
                }

                // Toggle fullscreen
                Action::Fullscreen => {
                    match self.selected_pane.selected() {
                        // These aren't fullscreenable. Still consume the event
                        // though, no one else will need it anyway
                        PrimaryPane::ProfileList | PrimaryPane::RecipeList => {}
                        PrimaryPane::Recipe => {
                            self.toggle_fullscreen(FullscreenMode::Request)
                        }
                        PrimaryPane::Response => {
                            self.toggle_fullscreen(FullscreenMode::Response)
                        }
                    }
                }
                // Exit fullscreen
                Action::Cancel if self.fullscreen_mode.is_some() => {
                    *self.fullscreen_mode = None;
                }
                _ => return Update::Propagate(event),
            },

            _ => return Update::Propagate(event),
        }
        Update::Consumed
    }

    fn children(&mut self) -> Vec<Component<&mut dyn EventHandler>> {
        let child = match (*self.fullscreen_mode, self.selected_pane.selected())
        {
            (Some(FullscreenMode::Request), _)
            | (None, PrimaryPane::Recipe) => self.request_pane.as_child(),
            (Some(FullscreenMode::Response), _)
            | (None, PrimaryPane::Response) => self.response_pane.as_child(),
            (None, PrimaryPane::ProfileList) => {
                self.profile_list_pane.as_child()
            }
            (None, PrimaryPane::RecipeList) => self.recipe_list_pane.as_child(),
        };
        vec![child]
    }
}

impl<'a> Draw<PrimaryViewProps<'a>> for PrimaryView {
    fn draw(&self, frame: &mut Frame, props: PrimaryViewProps<'a>, area: Rect) {
        match *self.fullscreen_mode {
            None => self.draw_all_panes(frame, props, area),
            Some(FullscreenMode::Request) => {
                self.request_pane.draw(
                    frame,
                    RecipePaneProps {
                        is_selected: true,
                        selected_recipe: self.selected_recipe(),
                        selected_profile_id: self
                            .selected_profile()
                            .map(|profile| &profile.id),
                    },
                    area,
                );
            }
            Some(FullscreenMode::Response) => {
                self.response_pane.draw(
                    frame,
                    ResponsePaneProps {
                        is_selected: true,
                        active_request: props.active_request,
                    },
                    area,
                );
            }
        }
    }
}
