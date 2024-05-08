//! Components for the "primary" view, which is the paned request/response view

use crate::{
    collection::{Collection, Profile, Recipe},
    tui::{
        input::Action,
        message::{Message, MessageSender, RequestConfig},
        view::{
            common::actions::ActionsModal,
            component::{
                help::HelpModal,
                profile_select::ProfilePane,
                recipe_list::{RecipeListPane, RecipeListPaneProps},
                recipe_pane::{RecipePane, RecipePaneProps},
                request_pane::{RequestPane, RequestPaneProps},
                response_pane::{ResponsePane, ResponsePaneProps},
            },
            draw::Draw,
            event::{Event, EventHandler, EventQueue, Update},
            state::{
                fixed_select::FixedSelectState,
                persistence::{Persistent, PersistentKey},
                RequestState,
            },
            Component,
        },
    },
};
use derive_more::Display;
use itertools::Itertools;
use ratatui::{
    layout::Layout,
    prelude::{Constraint, Rect},
    Frame,
};
use serde::{Deserialize, Serialize};
use strum::{EnumCount, EnumIter};

/// Primary TUI view, which shows request/response panes
#[derive(derive_more::Debug)]
pub struct PrimaryView {
    // Own state
    selected_pane: Persistent<FixedSelectState<PrimaryPane>>,
    fullscreen_mode: Persistent<Option<FullscreenMode>>,

    // Children
    #[debug(skip)]
    profile_pane: Component<ProfilePane>,
    #[debug(skip)]
    recipe_list_pane: Component<RecipeListPane>,
    #[debug(skip)]
    recipe_pane: Component<RecipePane>,
    #[debug(skip)]
    request_pane: Component<RequestPane>,
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
    Default,
    Display,
    EnumCount,
    EnumIter,
    PartialEq,
    Serialize,
    Deserialize,
)]
pub enum PrimaryPane {
    #[default]
    RecipeList,
    Recipe,
    Request,
    Response,
}

/// The various things that can be requested (haha get it, requested) to be
/// shown in fullscreen. If one of these is requested while not available, we
/// simply won't show it.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
enum FullscreenMode {
    /// Fullscreen the active request recipe
    Recipe,
    /// Fullscreen the active request
    Request,
    /// Fullscreen the active response
    Response,
}

/// Sentinel type for propagating an even that closes fullscreen mode
struct ExitFullscreen;

impl PrimaryView {
    pub fn new(collection: &Collection, messages_tx: MessageSender) -> Self {
        let profile_pane = ProfilePane::new(
            collection.profiles.values().cloned().collect_vec(),
        )
        .into();
        let recipe_list_pane = RecipeListPane::new(&collection.recipes).into();
        let selected_pane = FixedSelectState::builder()
            // Changing panes kicks us out of fullscreen
            .on_select(|_| EventQueue::push(Event::new_other(ExitFullscreen)))
            .build();

        Self {
            selected_pane: Persistent::new(
                PersistentKey::PrimaryPane,
                selected_pane,
            ),
            fullscreen_mode: Persistent::new(
                PersistentKey::FullscreenMode,
                None,
            ),

            recipe_list_pane,
            profile_pane,
            recipe_pane: RecipePane::new(messages_tx).into(),
            request_pane: Default::default(),
            response_pane: Default::default(),
        }
    }

    /// Which recipe in the recipe list is selected? `None` iff the list is
    /// empty OR a folder is selected.
    pub fn selected_recipe(&self) -> Option<&Recipe> {
        self.recipe_list_pane.data().selected_recipe()
    }

    /// Which profile in the list is selected? `None` iff the list is empty
    pub fn selected_profile(&self) -> Option<&Profile> {
        self.profile_pane.data().selected_profile()
    }

    /// Draw the "normal" view, when nothing is full
    fn draw_all_panes(
        &self,
        frame: &mut Frame,
        props: PrimaryViewProps,
        area: Rect,
    ) {
        // Split the main pane horizontally
        let [left_area, right_area] =
            Layout::horizontal([Constraint::Max(40), Constraint::Min(40)])
                .areas(area);

        let [profile_area, recipes_area] =
            Layout::vertical([Constraint::Length(3), Constraint::Min(0)])
                .areas(left_area);
        let [recipe_area, request_area, response_area] =
            self.get_right_column_layout(right_area);

        // Primary panes
        self.profile_pane.draw(frame, (), profile_area);
        self.recipe_list_pane.draw(
            frame,
            RecipeListPaneProps {
                is_selected: self.is_selected(PrimaryPane::RecipeList),
            },
            recipes_area,
        );

        self.recipe_pane.draw(
            frame,
            RecipePaneProps {
                is_selected: self.is_selected(PrimaryPane::Recipe),
                selected_recipe: self.selected_recipe(),
                selected_profile_id: self
                    .selected_profile()
                    .map(|profile| &profile.id),
            },
            recipe_area,
        );
        self.request_pane.draw(
            frame,
            RequestPaneProps {
                is_selected: self.is_selected(PrimaryPane::Request),
                active_request: props.active_request,
            },
            request_area,
        );
        self.response_pane.draw(
            frame,
            ResponsePaneProps {
                is_selected: self.is_selected(PrimaryPane::Response),
                active_request: props.active_request,
            },
            response_area,
        );
    }

    fn toggle_fullscreen(&mut self, mode: FullscreenMode) {
        // If we're already in the given mode, exit
        *self.fullscreen_mode = if Some(mode) == *self.fullscreen_mode {
            None
        } else {
            Some(mode)
        };
    }

    /// Is the given pane selected?
    fn is_selected(&self, primary_pane: PrimaryPane) -> bool {
        self.selected_pane.is_selected(&primary_pane)
    }

    /// Get layout for the right column of panes
    fn get_right_column_layout(&self, area: Rect) -> [Rect; 3] {
        // Split right column vertically. Expand the currently selected pane
        let (top, middle, bottom) = if self.is_selected(PrimaryPane::Recipe) {
            (3, 1, 1)
        } else if self.is_selected(PrimaryPane::Request) {
            (1, 3, 1)
        } else if self.is_selected(PrimaryPane::Response) {
            (1, 1, 3)
        } else {
            (1, 1, 1) // Default to even sizing
        };
        let denominator = top + middle + bottom;
        Layout::vertical([
            Constraint::Ratio(top, denominator),
            Constraint::Ratio(middle, denominator),
            Constraint::Ratio(bottom, denominator),
        ])
        .areas(area)
    }
}

impl EventHandler for PrimaryView {
    fn update(&mut self, messages_tx: &MessageSender, event: Event) -> Update {
        match &event {
            // Load latest request for selected recipe from database
            Event::HttpLoadRequest => {
                if let Some(recipe) = self.selected_recipe() {
                    messages_tx.send(Message::RequestLoad {
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
                    messages_tx.send(Message::HttpBeginRequest(
                        RequestConfig {
                            // Reach into the children to grab state (ugly!)
                            recipe_id: recipe.id.clone(),
                            profile_id: self
                                .selected_profile()
                                .map(|profile| profile.id.clone()),
                            options: self.recipe_pane.data().recipe_options(),
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
                    if self.recipe_list_pane.intersects(mouse) {
                        self.selected_pane.select(&PrimaryPane::RecipeList);
                    } else if self.recipe_pane.intersects(mouse) {
                        self.selected_pane.select(&PrimaryPane::Recipe);
                    } else if self.request_pane.intersects(mouse) {
                        self.selected_pane.select(&PrimaryPane::Request);
                    } else if self.response_pane.intersects(mouse) {
                        self.selected_pane.select(&PrimaryPane::Response);
                    }
                }
                Action::PreviousPane => self.selected_pane.previous(),
                Action::NextPane => self.selected_pane.next(),
                Action::Submit => {
                    // Send a request from anywhere
                    EventQueue::push(Event::HttpSendRequest);
                }
                Action::OpenActions => {
                    EventQueue::open_modal_default::<ActionsModal>();
                }
                Action::OpenHelp => {
                    EventQueue::open_modal_default::<HelpModal>();
                }
                Action::SelectRecipeList => {
                    self.selected_pane.select(&PrimaryPane::RecipeList)
                }
                Action::SelectRecipe => {
                    self.selected_pane.select(&PrimaryPane::Recipe)
                }
                Action::SelectRequest => {
                    self.selected_pane.select(&PrimaryPane::Request)
                }
                Action::SelectResponse => {
                    self.selected_pane.select(&PrimaryPane::Response)
                }

                // Toggle fullscreen
                Action::Fullscreen => {
                    match self.selected_pane.selected() {
                        // These aren't fullscreenable. Still consume the event
                        // though, no one else will need it anyway
                        PrimaryPane::RecipeList => {}
                        PrimaryPane::Recipe => {
                            self.toggle_fullscreen(FullscreenMode::Recipe)
                        }
                        PrimaryPane::Request => {
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

            Event::Other(event) => {
                if let Some(ExitFullscreen) = event.downcast_ref() {
                    *self.fullscreen_mode = None;
                }
            }

            _ => return Update::Propagate(event),
        }
        Update::Consumed
    }

    fn children(&mut self) -> Vec<Component<&mut dyn EventHandler>> {
        // Profile pane is always accessible because it's not actually
        // selectable, all it does it open its modal
        let mut children = vec![self.profile_pane.as_child()];
        let child = if let Some(fullscreen_mode) = *self.fullscreen_mode {
            match fullscreen_mode {
                FullscreenMode::Recipe => self.recipe_list_pane.as_child(),
                FullscreenMode::Request => self.request_pane.as_child(),
                FullscreenMode::Response => self.response_pane.as_child(),
            }
        } else {
            match self.selected_pane.selected() {
                PrimaryPane::RecipeList => self.recipe_list_pane.as_child(),
                PrimaryPane::Recipe => self.recipe_pane.as_child(),
                PrimaryPane::Request => self.request_pane.as_child(),
                PrimaryPane::Response => self.response_pane.as_child(),
            }
        };
        children.push(child);
        children
    }
}

impl<'a> Draw<PrimaryViewProps<'a>> for PrimaryView {
    fn draw(&self, frame: &mut Frame, props: PrimaryViewProps<'a>, area: Rect) {
        match *self.fullscreen_mode {
            None => self.draw_all_panes(frame, props, area),
            Some(FullscreenMode::Recipe) => self.recipe_pane.draw(
                frame,
                RecipePaneProps {
                    is_selected: true,
                    selected_recipe: self.selected_recipe(),
                    selected_profile_id: self
                        .selected_profile()
                        .map(|profile| &profile.id),
                },
                area,
            ),
            Some(FullscreenMode::Request) => self.request_pane.draw(
                frame,
                RequestPaneProps {
                    is_selected: true,
                    active_request: props.active_request,
                },
                area,
            ),
            Some(FullscreenMode::Response) => self.response_pane.draw(
                frame,
                ResponsePaneProps {
                    is_selected: true,
                    active_request: props.active_request,
                },
                area,
            ),
        }
    }
}
