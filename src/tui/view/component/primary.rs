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
            Component, ModalPriority,
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
#[derive(derive_more::Debug)]
pub struct PrimaryView {
    // Own state
    selected_pane: SelectState<Fixed, PrimaryPane, ListState>,
    fullscreen_mode: Option<FullscreenMode>,

    // Children
    #[debug(skip)]
    profile_list_pane: Component<ProfileListPane>,
    #[debug(skip)]
    recipe_list_pane: Component<RecipeListPane>,
    #[debug(skip)]
    request_pane: Component<RequestPane>,
    #[debug(skip)]
    response_pane: Component<ResponsePane>,
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

/// The various things that can be requested (haha get it, requested) to be
/// shown in fullscreen. If one of these is requested while not available, we
/// simply won't show it.
#[derive(Copy, Clone, Debug, PartialEq)]
enum FullscreenMode {
    /// Fullscreen the active request recipe
    Request,
    /// Fullscreen the active response
    Response,
}

impl PrimaryView {
    pub fn new(collection: &RequestCollection) -> Self {
        Self {
            selected_pane: Default::default(),
            fullscreen_mode: None,

            profile_list_pane: ProfileListPane::new(
                collection.profiles.to_owned(),
            )
            .into(),
            recipe_list_pane: RecipeListPane::new(
                collection.recipes.to_owned(),
            )
            .into(),
            request_pane: Default::default(),
            response_pane: Default::default(),
        }
    }

    /// Which recipe in the recipe list is selected? `None` iff the list is
    /// empty.
    pub fn selected_recipe(&self) -> Option<&RequestRecipe> {
        self.recipe_list_pane.recipes.selected()
    }

    /// Which profile in the list is selected? `None` iff the list is empty.
    /// Exposing inner state is hacky but it's an easy shortcut
    fn selected_profile(&self) -> Option<&Profile> {
        self.profile_list_pane.profiles.selected()
    }

    fn toggle_fullscreen(&mut self, mode: FullscreenMode) {
        // If we're already in the given mode, exit
        self.fullscreen_mode = if Some(mode) == self.fullscreen_mode {
            None
        } else {
            Some(mode)
        };
    }
}

impl EventHandler for PrimaryView {
    fn update(&mut self, context: &mut UpdateContext, event: Event) -> Update {
        match &event {
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
                            .select(context, &PrimaryPane::Request);
                    } else if self.response_pane.intersects(mouse) {
                        self.selected_pane
                            .select(context, &PrimaryPane::Response);
                    }
                    Update::Consumed
                }
                Action::PreviousPane => {
                    self.selected_pane.previous(context);
                    Update::Consumed
                }
                Action::NextPane => {
                    self.selected_pane.next(context);
                    Update::Consumed
                }
                Action::SendRequest => {
                    // Send a request from anywhere
                    context.queue_event(Event::HttpSendRequest);
                    Update::Consumed
                }
                Action::OpenSettings => {
                    context.queue_event(Event::OpenModal {
                        modal: Box::<SettingsModal>::default(),
                        priority: ModalPriority::Low,
                    });
                    Update::Consumed
                }
                Action::OpenHelp => {
                    context.queue_event(Event::OpenModal {
                        modal: Box::<HelpModal>::default(),
                        priority: ModalPriority::Low,
                    });
                    Update::Consumed
                }

                // Toggle fullscreen
                Action::Fullscreen => {
                    match self.selected_pane.selected() {
                        // These aren't fullscreenable. Still consume the event
                        // though, no one else will need it anyway
                        PrimaryPane::ProfileList | PrimaryPane::RecipeList => {}
                        PrimaryPane::Request => {
                            self.toggle_fullscreen(FullscreenMode::Request)
                        }
                        PrimaryPane::Response => {
                            self.toggle_fullscreen(FullscreenMode::Response)
                        }
                    }
                    Update::Consumed
                }
                // Exit fullscreen
                Action::Cancel if self.fullscreen_mode.is_some() => {
                    self.fullscreen_mode = None;
                    Update::Consumed
                }
                _ => Update::Propagate(event),
            },

            _ => Update::Propagate(event),
        }
    }

    fn children(&mut self) -> Vec<Component<&mut dyn EventHandler>> {
        let child = match (self.fullscreen_mode, self.selected_pane.selected())
        {
            (Some(FullscreenMode::Request), _)
            | (None, PrimaryPane::Request) => self.request_pane.as_child(),
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
    fn draw(
        &self,
        context: &mut DrawContext,
        props: PrimaryViewProps<'a>,
        area: Rect,
    ) {
        match self.fullscreen_mode {
            // Show all panes
            None => {
                // Split the main pane horizontally
                let [left_area, right_area] = layout(
                    area,
                    Direction::Horizontal,
                    [Constraint::Max(40), Constraint::Percentage(50)],
                );

                // Split left column vertically
                let [profiles_area, recipes_area] = layout(
                    left_area,
                    Direction::Vertical,
                    [
                        // Make profile list as small as possible, with a max
                        // size
                        Constraint::Max(
                            self.profile_list_pane.profiles.len().clamp(1, 16)
                                as u16
                                + 2, // Account for top/bottom border
                        ),
                        Constraint::Min(0),
                    ],
                );

                // Split right column vertically
                let [request_area, response_area] = layout(
                    right_area,
                    Direction::Vertical,
                    [Constraint::Percentage(50), Constraint::Percentage(50)],
                );

                // Primary panes
                let panes = &self.selected_pane;
                self.profile_list_pane.draw(
                    context,
                    ListPaneProps {
                        is_selected: panes
                            .is_selected(&PrimaryPane::ProfileList),
                    },
                    profiles_area,
                );
                self.recipe_list_pane.draw(
                    context,
                    ListPaneProps {
                        is_selected: panes
                            .is_selected(&PrimaryPane::RecipeList),
                    },
                    recipes_area,
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
                    request_area,
                );
                self.response_pane.draw(
                    context,
                    ResponsePaneProps {
                        is_selected: panes.is_selected(&PrimaryPane::Response),
                        active_request: props.active_request,
                    },
                    response_area,
                );
            }
            Some(FullscreenMode::Request) => {
                self.request_pane.draw(
                    context,
                    RequestPaneProps {
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
                    context,
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

#[derive(Debug)]
struct ProfileListPane {
    profiles: Component<SelectState<Dynamic, Profile>>,
}

struct ListPaneProps {
    is_selected: bool,
}

impl ProfileListPane {
    pub fn new(profiles: Vec<Profile>) -> Self {
        Self {
            profiles: SelectState::new(profiles).into(),
        }
    }
}

impl EventHandler for ProfileListPane {
    fn children(&mut self) -> Vec<Component<&mut dyn EventHandler>> {
        vec![self.profiles.as_child()]
    }
}

impl Draw<ListPaneProps> for ProfileListPane {
    fn draw(
        &self,
        context: &mut DrawContext,
        props: ListPaneProps,
        area: Rect,
    ) {
        self.profiles.set_area(area); // Needed for tracking cursor events
        let list = List {
            block: Block {
                title: &PrimaryPane::ProfileList.to_string(),
                is_focused: props.is_selected,
            },
            list: &self.profiles,
        };
        context.frame.render_stateful_widget(
            list.generate(),
            area,
            &mut self.profiles.state_mut(),
        )
    }
}

#[derive(Debug)]
struct RecipeListPane {
    recipes: Component<SelectState<Dynamic, RequestRecipe>>,
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
                .on_submit(on_submit)
                .into(),
        }
    }
}

impl EventHandler for RecipeListPane {
    fn children(&mut self) -> Vec<Component<&mut dyn EventHandler>> {
        vec![self.recipes.as_child()]
    }
}

impl Draw<ListPaneProps> for RecipeListPane {
    fn draw(
        &self,
        context: &mut DrawContext,
        props: ListPaneProps,
        area: Rect,
    ) {
        self.recipes.set_area(area); // Needed for tracking cursor events
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
            area,
            &mut self.recipes.state_mut(),
        )
    }
}
