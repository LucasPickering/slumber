//! Components for the "primary" view, which is the paned request/response view

use crate::{
    message::{Message, RequestConfig},
    view::{
        common::actions::ActionsModal,
        component::{
            exchange_pane::{ExchangePane, ExchangePaneProps},
            help::HelpModal,
            profile_select::ProfilePane,
            recipe_list::RecipeListPane,
            recipe_pane::{RecipeMenuAction, RecipePane, RecipePaneProps},
        },
        context::{Persisted, PersistedLazy},
        draw::{Draw, DrawMetadata, ToStringGenerate},
        event::{Event, EventHandler, Update},
        state::{fixed_select::FixedSelectState, RequestState},
        Component, ViewContext,
    },
};
use derive_more::Display;
use itertools::Itertools;
use persisted::SingletonKey;
use ratatui::{
    layout::Layout,
    prelude::{Constraint, Rect},
    Frame,
};
use serde::{Deserialize, Serialize};
use slumber_config::Action;
use slumber_core::collection::{
    Collection, Profile, ProfileId, Recipe, RecipeId, RecipeNode,
};
use strum::{EnumCount, EnumIter};

/// Primary TUI view, which shows request/response panes
#[derive(Debug)]
pub struct PrimaryView {
    // Own state
    selected_pane:
        PersistedLazy<SingletonKey<PrimaryPane>, FixedSelectState<PrimaryPane>>,
    fullscreen_mode: Persisted<FullscreenModeKey>,

    // Children
    profile_pane: Component<ProfilePane>,
    recipe_list_pane: Component<RecipeListPane>,
    recipe_pane: Component<RecipePane>,
    exchange_pane: Component<ExchangePane>,
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
    Exchange,
}

/// Panes that can be fullscreened. This is separate from [PrimaryPane] because
/// it makes it easy to check when we should exit fullscreen mode.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
enum FullscreenMode {
    /// Fullscreen the active request recipe
    Recipe,
    /// Fullscreen the active request/response exchange
    Exchange,
}

/// Persistence key for fullscreen mode
#[derive(Debug, Default, persisted::PersistedKey, Serialize)]
#[persisted(Option<FullscreenMode>)]
struct FullscreenModeKey;

/// Event triggered when selected pane changes, so we can exit fullscreen
#[derive(Debug)]
struct PaneChanged;

/// Action menu items. This is the fallback menu if none of our children have
/// one
#[derive(
    Copy, Clone, Debug, Default, Display, EnumCount, EnumIter, PartialEq,
)]
enum MenuAction {
    #[default]
    #[display("Edit Collection")]
    EditCollection,
}
impl ToStringGenerate for MenuAction {}

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
                ViewContext::push_event(Event::new_local(PaneChanged))
            })
            .build();

        Self {
            selected_pane: PersistedLazy::new(
                SingletonKey::default(),
                selected_pane,
            ),
            fullscreen_mode: Persisted::default(),

            recipe_list_pane,
            profile_pane,
            recipe_pane: Default::default(),
            exchange_pane: Default::default(),
        }
    }

    /// Which recipe in the recipe list is selected? `None` iff the list is
    /// empty OR a folder is selected.
    pub fn selected_recipe(&self) -> Option<&Recipe> {
        self.recipe_list_pane
            .data()
            .selected_node()
            .and_then(RecipeNode::recipe)
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

    /// Draw the "normal" view, when nothing is fullscreened
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

        let selected_recipe_node = self.recipe_list_pane.data().selected_node();
        self.recipe_pane.draw(
            frame,
            RecipePaneProps {
                selected_recipe_node,
                selected_profile_id: self.selected_profile_id(),
            },
            recipe_area,
            self.is_selected(PrimaryPane::Recipe),
        );

        self.exchange_pane.draw(
            frame,
            ExchangePaneProps {
                selected_recipe_node,
                request_state: props.selected_request,
            },
            request_response_area,
            self.is_selected(PrimaryPane::Exchange),
        );
    }

    /// Is the given pane selected?
    fn is_selected(&self, primary_pane: PrimaryPane) -> bool {
        self.selected_pane.is_selected(&primary_pane)
    }

    fn toggle_fullscreen(&mut self, mode: FullscreenMode) {
        // If we're already in the given mode, exit
        *self.fullscreen_mode = if Some(mode) == *self.fullscreen_mode {
            None
        } else {
            Some(mode)
        };
    }

    /// Exit fullscreen mode if it doesn't match the selected pane. This is
    /// called when the pane changes, but it's possible they match when we're
    /// loading from persistence. In those cases, stay in fullscreen.
    fn maybe_exit_fullscreen(&mut self) {
        match (self.selected_pane.selected(), *self.fullscreen_mode) {
            (PrimaryPane::Recipe, Some(FullscreenMode::Recipe))
            | (PrimaryPane::Exchange, Some(FullscreenMode::Exchange)) => {}
            _ => *self.fullscreen_mode = None,
        }
    }

    /// Get layout for the right column of panes
    fn get_right_column_layout(&self, area: Rect) -> [Rect; 2] {
        // Split right column vertically. Expand the currently selected pane
        let (top, bottom) = match self.selected_pane.selected() {
            PrimaryPane::Recipe => (2, 1),
            PrimaryPane::Exchange | PrimaryPane::RecipeList => (1, 2),
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
            options: self.recipe_pane.data().build_options(),
        };
        let message = match action {
            RecipeMenuAction::EditCollection => Message::CollectionEdit,
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
                                    .build_options(),
                            },
                        ));
                    }
                }
                Action::OpenActions => {
                    ViewContext::open_modal_default::<ActionsModal<MenuAction>>(
                    );
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
                    self.selected_pane.select(&PrimaryPane::Exchange)
                }

                // Toggle fullscreen
                Action::Fullscreen => {
                    match self.selected_pane.selected() {
                        PrimaryPane::Recipe => {
                            self.toggle_fullscreen(FullscreenMode::Recipe)
                        }
                        PrimaryPane::Exchange => {
                            self.toggle_fullscreen(FullscreenMode::Exchange)
                        }
                        // This isn't fullscreenable. Still consume the event
                        // though, no one else will need it anyway
                        PrimaryPane::RecipeList => {}
                    }
                }
                // Exit fullscreen
                Action::Cancel if self.fullscreen_mode.is_some() => {
                    *self.fullscreen_mode = None;
                }
                _ => return Update::Propagate(event),
            },

            Event::Local(local) => {
                if let Some(PaneChanged) = local.downcast_ref() {
                    self.maybe_exit_fullscreen();
                } else if let Some(pane) = local.downcast_ref::<PrimaryPane>() {
                    // Children can select themselves by sending PrimaryPane
                    self.selected_pane.select(pane);
                } else if let Some(action) =
                    local.downcast_ref::<RecipeMenuAction>()
                {
                    self.handle_recipe_menu_action(*action);
                } else if let Some(action) = local.downcast_ref::<MenuAction>()
                {
                    match action {
                        MenuAction::EditCollection => {
                            ViewContext::send_message(Message::CollectionEdit)
                        }
                    }
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
            self.exchange_pane.as_child(),
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
                    selected_recipe_node: self
                        .recipe_list_pane
                        .data()
                        .selected_node(),
                    selected_profile_id: self.selected_profile_id(),
                },
                metadata.area(),
                true,
            ),
            Some(FullscreenMode::Exchange) => self.exchange_pane.draw(
                frame,
                ExchangePaneProps {
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
        message::{Message, RequestConfig},
        test_util::{harness, TestHarness},
        view::test_util::TestComponent,
    };
    use persisted::PersistedStore;
    use rstest::rstest;
    use slumber_core::{
        assert_matches, http::BuildOptions, test_util::Factory,
    };

    /// Create component to be tested
    fn create_component(
        harness: TestHarness,
        collection: &Collection,
    ) -> TestComponent<PrimaryView, PrimaryViewProps<'static>> {
        let mut component = TestComponent::new(
            harness,
            PrimaryView::new(collection),
            PrimaryViewProps {
                selected_request: None,
            },
        );
        // Clear template preview messages so we can test what we want
        component.harness_mut().clear_messages();
        component
    }

    /// Test selected pane and fullscreen mode loading from persistence
    #[rstest]
    fn test_pane_persistence(harness: TestHarness) {
        ViewContext::store_persisted(
            &SingletonKey::<PrimaryPane>::default(),
            PrimaryPane::Exchange,
        );
        ViewContext::store_persisted(
            &FullscreenModeKey,
            Some(FullscreenMode::Exchange),
        );

        let collection = Collection::factory(());
        let component = create_component(harness, &collection);
        assert_eq!(
            component.data().selected_pane.selected(),
            PrimaryPane::Exchange
        );
        assert_matches!(
            *component.data().fullscreen_mode,
            Some(FullscreenMode::Exchange)
        );
    }

    /// Test "Copy URL" action, which is available via the Recipe List or Recipe
    /// panes
    #[rstest]
    fn test_copy_url(harness: TestHarness) {
        let collection = Collection::factory(());
        let mut component = create_component(harness, &collection);
        component
            .update_draw(Event::new_local(RecipeMenuAction::CopyUrl))
            .assert_empty();

        let request_config = assert_matches!(
            component.harness_mut().pop_message_now(),
            Message::CopyRequestUrl(request_config) => request_config,
        );
        assert_eq!(
            request_config,
            RequestConfig {
                recipe_id: collection.first_recipe_id().clone(),
                profile_id: Some(collection.first_profile_id().clone()),
                options: BuildOptions::default()
            }
        );
    }

    /// Test "Copy Body" action, which is available via the Recipe List or
    /// Recipe panes
    #[rstest]
    fn test_copy_body(harness: TestHarness) {
        let collection = Collection::factory(());
        let mut component = create_component(harness, &collection);
        component
            .update_draw(Event::new_local(RecipeMenuAction::CopyBody))
            .assert_empty();

        let request_config = assert_matches!(
            component.harness_mut().pop_message_now(),
            Message::CopyRequestBody(request_config) => request_config,
        );
        assert_eq!(
            request_config,
            RequestConfig {
                recipe_id: collection.first_recipe_id().clone(),
                profile_id: Some(collection.first_profile_id().clone()),
                options: BuildOptions::default()
            }
        );
    }

    /// Test "Copy as cURL" action, which is available via the Recipe List or
    /// Recipe panes
    #[rstest]
    fn test_copy_as_curl(harness: TestHarness) {
        let collection = Collection::factory(());
        let mut component = create_component(harness, &collection);
        component
            .update_draw(Event::new_local(RecipeMenuAction::CopyCurl))
            .assert_empty();

        let request_config = assert_matches!(
            component.harness_mut().pop_message_now(),
            Message::CopyRequestCurl(request_config) => request_config,
        );
        assert_eq!(
            request_config,
            RequestConfig {
                recipe_id: collection.first_recipe_id().clone(),
                profile_id: Some(collection.first_profile_id().clone()),
                options: BuildOptions::default()
            }
        );
    }
}
