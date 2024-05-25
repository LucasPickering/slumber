//! Components for the "primary" view, which is the paned request/response view

use crate::{
    collection::{Collection, Profile, ProfileId, Recipe, RecipeId},
    tui::{
        input::Action,
        message::{Message, RequestConfig},
        view::{
            common::actions::ActionsModal,
            component::{
                help::HelpModal,
                profile_select::ProfilePane,
                recipe_list::RecipeListPane,
                recipe_pane::{RecipeMenuAction, RecipePane, RecipePaneProps},
                record_pane::{RecordPane, RecordPaneProps},
            },
            draw::{Draw, DrawMetadata},
            event::{Event, EventHandler, Update},
            state::{
                fixed_select::{FixedSelect, FixedSelectState},
                persistence::{
                    impl_persistable, Persistable, Persistent, PersistentKey,
                },
                RequestState,
            },
            Component, ViewContext,
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
#[derive(Debug)]
pub struct PrimaryView {
    // Own state
    selected_pane: Persistent<FixedSelectState<PrimaryPane>>,
    fullscreen_mode: Persistent<Option<FullscreenMode>>,

    // Children
    profile_pane: Component<ProfilePane>,
    recipe_list_pane: Component<RecipeListPane>,
    recipe_pane: Component<RecipePane>,
    record_pane: Component<RecordPane>,
}

#[cfg_attr(test, derive(Clone))]
pub struct PrimaryViewProps<'a> {
    pub selected_request: Option<&'a RequestState>,
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
    Record,
}
impl FixedSelect for PrimaryPane {}

/// The various things that can be requested (haha get it, requested) to be
/// shown in fullscreen. If one of these is requested while not available, we
/// simply won't show it.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
enum FullscreenMode {
    /// Fullscreen the active request recipe
    Recipe,
    /// Fullscreen the active request/response
    Record,
}
impl_persistable!(Option<FullscreenMode>);

/// Sentinel type for propagating an even that closes fullscreen mode
struct ExitFullscreen;

impl PrimaryView {
    pub fn new(collection: &Collection) -> Self {
        let profile_pane = ProfilePane::new(
            collection.profiles.values().cloned().collect_vec(),
        )
        .into();
        let recipe_list_pane = RecipeListPane::new(&collection.recipes).into();
        let selected_pane = FixedSelectState::builder()
            // Changing panes kicks us out of fullscreen
            .on_select(|_| {
                ViewContext::push_event(Event::new_other(ExitFullscreen))
            })
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
            recipe_pane: Default::default(),
            record_pane: Default::default(),
        }
    }

    /// Which recipe in the recipe list is selected? `None` iff the list is
    /// empty OR a folder is selected.
    pub fn selected_recipe(&self) -> Option<&Recipe> {
        self.recipe_list_pane.data().selected_recipe()
    }

    pub fn selected_recipe_id(&self) -> Option<&RecipeId> {
        self.selected_recipe().map(|recipe| &recipe.id)
    }

    /// Which profile in the list is selected? `None` iff the list is empty
    pub fn selected_profile(&self) -> Option<&Profile> {
        self.profile_pane.data().selected_profile()
    }
    /// ID of the selected profile. `None` iff the list is empty
    pub fn selected_profile_id(&self) -> Option<&ProfileId> {
        self.selected_profile().map(|profile| &profile.id)
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
        let [recipe_area, request_response_area] =
            self.get_right_column_layout(right_area);

        self.profile_pane.draw(frame, (), profile_area, true);
        self.recipe_list_pane.draw(
            frame,
            (),
            recipes_area,
            self.is_selected(PrimaryPane::RecipeList),
        );

        self.recipe_pane.draw(
            frame,
            RecipePaneProps {
                selected_recipe: self.selected_recipe(),
                selected_profile_id: self.selected_profile_id(),
            },
            recipe_area,
            self.is_selected(PrimaryPane::Recipe),
        );

        self.record_pane.draw(
            frame,
            RecordPaneProps {
                selected_recipe_node: self
                    .recipe_list_pane
                    .data()
                    .selected_node(),
                request_state: props.selected_request,
            },
            request_response_area,
            self.is_selected(PrimaryPane::Record),
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
    fn get_right_column_layout(&self, area: Rect) -> [Rect; 2] {
        // Split right column vertically. Expand the currently selected pane
        let (top, bottom) = match self.selected_pane.selected() {
            PrimaryPane::Recipe => (2, 1),
            PrimaryPane::Record | PrimaryPane::RecipeList => (1, 2),
        };
        let denominator = top + bottom;
        Layout::vertical([
            Constraint::Ratio(top, denominator),
            Constraint::Ratio(bottom, denominator),
        ])
        .areas(area)
    }

    /// Handle menu actions for recipe list or detail panes. We handle this here
    /// for code de-duplication, and because we have access to all the needed
    /// context.
    fn handle_recipe_menu_action(&self, action: RecipeMenuAction) {
        // If no recipes are available, we can't do anything
        let Some(recipe_id) = self.selected_recipe_id().cloned() else {
            return;
        };

        let request_config = RequestConfig {
            profile_id: self.selected_profile_id().cloned(),
            recipe_id,
            options: self.recipe_pane.data().recipe_options(),
        };
        let message = match action {
            RecipeMenuAction::CopyUrl => {
                Message::CopyRequestUrl(request_config)
            }
            RecipeMenuAction::CopyBody => {
                Message::CopyRequestBody(request_config)
            }
            RecipeMenuAction::CopyCurl => {
                Message::CopyRequestCurl(request_config)
            }
        };
        ViewContext::send_message(message);
    }
}

impl EventHandler for PrimaryView {
    fn update(&mut self, event: Event) -> Update {
        match &event {
            // Input messages
            Event::Input {
                action: Some(action),
                event: _,
            } => match action {
                Action::PreviousPane => self.selected_pane.previous(),
                Action::NextPane => self.selected_pane.next(),
                Action::Submit => {
                    // Send a request from anywhere
                    if let Some(recipe_id) = self.selected_recipe_id() {
                        ViewContext::send_message(Message::HttpBeginRequest(
                            RequestConfig {
                                recipe_id: recipe_id.clone(),
                                profile_id: self.selected_profile_id().cloned(),
                                options: self
                                    .recipe_pane
                                    .data()
                                    .recipe_options(),
                            },
                        ));
                    }
                }
                Action::OpenActions => {
                    ViewContext::open_modal_default::<ActionsModal>();
                }
                Action::OpenHelp => {
                    ViewContext::open_modal_default::<HelpModal>();
                }

                // Pane hotkeys
                Action::SelectProfileList => {
                    self.profile_pane.data().open_modal()
                }
                Action::SelectRecipeList => {
                    self.selected_pane.select(&PrimaryPane::RecipeList)
                }
                Action::SelectRecipe => {
                    self.selected_pane.select(&PrimaryPane::Recipe)
                }
                Action::SelectResponse => {
                    self.selected_pane.select(&PrimaryPane::Record)
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
                        PrimaryPane::Record => {
                            self.toggle_fullscreen(FullscreenMode::Record)
                        }
                    }
                }
                // Exit fullscreen
                Action::Cancel if self.fullscreen_mode.is_some() => {
                    *self.fullscreen_mode = None;
                }
                _ => return Update::Propagate(event),
            },

            Event::Other(other) => {
                if let Some(ExitFullscreen) = other.downcast_ref() {
                    *self.fullscreen_mode = None;
                } else if let Some(pane) = other.downcast_ref::<PrimaryPane>() {
                    // Children can select themselves by sending PrimaryPane
                    self.selected_pane.select(pane);
                } else if let Some(action) =
                    other.downcast_ref::<RecipeMenuAction>()
                {
                    self.handle_recipe_menu_action(*action);
                } else {
                    return Update::Propagate(event);
                }
            }

            _ => return Update::Propagate(event),
        }
        Update::Consumed
    }

    fn children(&mut self) -> Vec<Component<&mut dyn EventHandler>> {
        vec![
            self.profile_pane.as_child(),
            self.recipe_list_pane.as_child(),
            self.recipe_pane.as_child(),
            self.record_pane.as_child(),
        ]
    }
}

impl<'a> Draw<PrimaryViewProps<'a>> for PrimaryView {
    fn draw(
        &self,
        frame: &mut Frame,
        props: PrimaryViewProps<'a>,
        metadata: DrawMetadata,
    ) {
        match *self.fullscreen_mode {
            None => self.draw_all_panes(frame, props, metadata.area()),
            Some(FullscreenMode::Recipe) => self.recipe_pane.draw(
                frame,
                RecipePaneProps {
                    selected_recipe: self.selected_recipe(),
                    selected_profile_id: self.selected_profile_id(),
                },
                metadata.area(),
                true,
            ),
            Some(FullscreenMode::Record) => self.record_pane.draw(
                frame,
                RecordPaneProps {
                    selected_recipe_node: self
                        .recipe_list_pane
                        .data()
                        .selected_node(),
                    request_state: props.selected_request,
                },
                metadata.area(),
                true,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        http::RecipeOptions,
        test_util::{assert_matches, Factory},
        tui::{
            message::{Message, RequestConfig},
            test_util::{harness, TestHarness},
            view::test_util::TestComponent,
        },
    };
    use rstest::{fixture, rstest};

    /// Create component to be tested
    #[fixture]
    fn component(
        harness: TestHarness,
    ) -> TestComponent<PrimaryView, PrimaryViewProps<'static>> {
        let collection = Collection::factory(());
        let mut component = TestComponent::new(
            harness,
            PrimaryView::new(&collection),
            PrimaryViewProps {
                selected_request: None,
            },
        );
        // Clear template preview messages so we can test what we want
        component.harness_mut().clear_messages();
        component
    }

    /// Test "Copy URL" action, which is available via the Recipe List or Recipe
    /// panes
    #[rstest]
    fn test_copy_url(
        mut component: TestComponent<PrimaryView, PrimaryViewProps<'static>>,
    ) {
        component
            .update_draw(Event::new_other(RecipeMenuAction::CopyUrl))
            .assert_empty();

        let request_config = assert_matches!(
            component.harness_mut().pop_message_now(),
            Message::CopyRequestUrl(request_config) => request_config,
        );
        assert_eq!(
            request_config,
            RequestConfig {
                recipe_id: "recipe1".into(),
                profile_id: Some("profile1".into()),
                options: RecipeOptions::default()
            }
        );
    }

    /// Test "Copy Body" action, which is available via the Recipe List or
    /// Recipe panes
    #[rstest]
    fn test_copy_body(
        mut component: TestComponent<PrimaryView, PrimaryViewProps<'static>>,
    ) {
        component
            .update_draw(Event::new_other(RecipeMenuAction::CopyBody))
            .assert_empty();

        let request_config = assert_matches!(
            component.harness_mut().pop_message_now(),
            Message::CopyRequestBody(request_config) => request_config,
        );
        assert_eq!(
            request_config,
            RequestConfig {
                recipe_id: "recipe1".into(),
                profile_id: Some("profile1".into()),
                options: RecipeOptions::default()
            }
        );
    }

    /// Test "Copy as cURL" action, which is available via the Recipe List or
    /// Recipe panes
    #[rstest]
    fn test_copy_as_curl(
        mut component: TestComponent<PrimaryView, PrimaryViewProps<'static>>,
    ) {
        component
            .update_draw(Event::new_other(RecipeMenuAction::CopyCurl))
            .assert_empty();

        let request_config = assert_matches!(
            component.harness_mut().pop_message_now(),
            Message::CopyRequestCurl(request_config) => request_config,
        );
        assert_eq!(
            request_config,
            RequestConfig {
                recipe_id: "recipe1".into(),
                profile_id: Some("profile1".into()),
                options: RecipeOptions::default()
            }
        );
    }
}
