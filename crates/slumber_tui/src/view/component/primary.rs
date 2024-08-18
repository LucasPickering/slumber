//! Components for the "primary" view, which is the paned request/response view

use crate::{
    message::Message,
    util::ResultReported,
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
        event::{Child, Event, EventHandler, Update},
        state::{fixed_select::FixedSelectState, RequestState},
        Component, ViewContext,
    },
};
use derive_more::Display;
use persisted::SingletonKey;
use ratatui::{
    layout::Layout,
    prelude::{Constraint, Rect},
    Frame,
};
use serde::{Deserialize, Serialize};
use slumber_config::Action;
use slumber_core::collection::{
    Collection, ProfileId, RecipeId, RecipeNodeDiscriminants,
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
        let profile_pane = ProfilePane::new(&collection.profiles).into();
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
    pub fn selected_recipe_id(&self) -> Option<&RecipeId> {
        self.recipe_list_pane
            .data()
            .selected_node()
            .and_then(|(id, kind)| {
                if matches!(kind, RecipeNodeDiscriminants::Recipe) {
                    Some(id)
                } else {
                    None
                }
            })
    }

    /// ID of the selected profile. `None` iff the list is empty
    pub fn selected_profile_id(&self) -> Option<&ProfileId> {
        self.profile_pane.data().selected_profile_id()
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

        let (selected_recipe_id, selected_recipe_kind) =
            match self.recipe_list_pane.data().selected_node() {
                Some((selected_recipe_id, selected_recipe_kind)) => {
                    (Some(selected_recipe_id), Some(selected_recipe_kind))
                }
                None => (None, None),
            };
        let collection = ViewContext::collection();
        let selected_recipe_node = selected_recipe_id.and_then(|id| {
            collection
                .recipes
                .try_get(id)
                .reported(&ViewContext::messages_tx())
        });
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
                selected_recipe_kind,
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
        *self.fullscreen_mode.get_mut() = if Some(mode) == *self.fullscreen_mode
        {
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
            _ => *self.fullscreen_mode.get_mut() = None,
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
        let Some(config) = self.recipe_pane.data().request_config() else {
            return;
        };

        let message = match action {
            RecipeMenuAction::EditCollection => Message::CollectionEdit,
            RecipeMenuAction::CopyUrl => Message::CopyRequestUrl(config),
            RecipeMenuAction::CopyBody => Message::CopyRequestBody(config),
            RecipeMenuAction::CopyCurl => Message::CopyRequestCurl(config),
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
                Action::PreviousPane => self.selected_pane.get_mut().previous(),
                Action::NextPane => self.selected_pane.get_mut().next(),
                Action::Submit => {
                    // Send a request from anywhere
                    if let Some(config) =
                        self.recipe_pane.data().request_config()
                    {
                        ViewContext::send_message(Message::HttpBeginRequest(
                            config,
                        ));
                    }
                }
                Action::OpenActions => {
                    ViewContext::open_modal::<ActionsModal<MenuAction>>(
                        Default::default(),
                    );
                }
                Action::OpenHelp => {
                    ViewContext::open_modal::<HelpModal>(Default::default());
                }

                // Pane hotkeys
                Action::SelectProfileList => {
                    self.profile_pane.data().open_modal()
                }
                Action::SelectRecipeList => self
                    .selected_pane
                    .get_mut()
                    .select(&PrimaryPane::RecipeList),
                Action::SelectRecipe => {
                    self.selected_pane.get_mut().select(&PrimaryPane::Recipe)
                }
                Action::SelectResponse => {
                    self.selected_pane.get_mut().select(&PrimaryPane::Exchange)
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
                    *self.fullscreen_mode.get_mut() = None;
                }
                _ => return Update::Propagate(event),
            },

            Event::Local(local) => {
                if let Some(PaneChanged) = local.downcast_ref() {
                    self.maybe_exit_fullscreen();
                } else if let Some(pane) = local.downcast_ref::<PrimaryPane>() {
                    // Children can select themselves by sending PrimaryPane
                    self.selected_pane.get_mut().select(pane);
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

    fn children(&mut self) -> Vec<Component<Child<'_>>> {
        vec![
            self.profile_pane.to_child_mut(),
            self.recipe_list_pane.to_child_mut(),
            self.recipe_pane.to_child_mut(),
            self.exchange_pane.to_child_mut(),
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
            Some(FullscreenMode::Recipe) => {
                let collection = ViewContext::collection();
                let selected_recipe_node =
                    self.recipe_list_pane.data().selected_node().and_then(
                        |(id, _)| {
                            collection
                                .recipes
                                .try_get(id)
                                .reported(&ViewContext::messages_tx())
                        },
                    );
                self.recipe_pane.draw(
                    frame,
                    RecipePaneProps {
                        selected_recipe_node,
                        selected_profile_id: self.selected_profile_id(),
                    },
                    metadata.area(),
                    self.is_selected(PrimaryPane::Recipe),
                );
            }
            Some(FullscreenMode::Exchange) => self.exchange_pane.draw(
                frame,
                ExchangePaneProps {
                    selected_recipe_kind: self
                        .recipe_list_pane
                        .data()
                        .selected_node()
                        .map(|(_, kind)| kind),
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
        test_util::{harness, terminal, TestHarness, TestTerminal},
        view::test_util::TestComponent,
    };
    use persisted::PersistedStore;
    use rstest::rstest;
    use slumber_core::{assert_matches, http::BuildOptions};

    /// Create component to be tested
    fn create_component<'term>(
        harness: &mut TestHarness,
        terminal: &'term TestTerminal,
    ) -> TestComponent<'term, PrimaryView, PrimaryViewProps<'static>> {
        let view = PrimaryView::new(&harness.collection);
        let component = TestComponent::new(
            terminal,
            view,
            PrimaryViewProps {
                selected_request: None,
            },
        );
        // Clear template preview messages so we can test what we want
        harness.clear_messages();
        component
    }

    /// Test selected pane and fullscreen mode loading from persistence
    #[rstest]
    fn test_pane_persistence(mut harness: TestHarness, terminal: TestTerminal) {
        ViewContext::store_persisted(
            &SingletonKey::<PrimaryPane>::default(),
            &PrimaryPane::Exchange,
        );
        ViewContext::store_persisted(
            &FullscreenModeKey,
            &Some(FullscreenMode::Exchange),
        );

        let component = create_component(&mut harness, &terminal);
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
    fn test_copy_url(mut harness: TestHarness, terminal: TestTerminal) {
        let expected_config = RequestConfig {
            recipe_id: harness.collection.first_recipe_id().clone(),
            profile_id: Some(harness.collection.first_profile_id().clone()),
            options: BuildOptions::default(),
        };
        let mut component = create_component(&mut harness, &terminal);
        component
            .update_draw(Event::new_local(RecipeMenuAction::CopyUrl))
            .assert_empty();

        let request_config = assert_matches!(
            harness.pop_message_now(),
            Message::CopyRequestUrl(request_config) => request_config,
        );
        assert_eq!(request_config, expected_config);
    }

    /// Test "Copy Body" action, which is available via the Recipe List or
    /// Recipe panes
    #[rstest]
    fn test_copy_body(mut harness: TestHarness, terminal: TestTerminal) {
        let expected_config = RequestConfig {
            recipe_id: harness.collection.first_recipe_id().clone(),
            profile_id: Some(harness.collection.first_profile_id().clone()),
            options: BuildOptions::default(),
        };
        let mut component = create_component(&mut harness, &terminal);
        component
            .update_draw(Event::new_local(RecipeMenuAction::CopyBody))
            .assert_empty();

        let request_config = assert_matches!(
            harness.pop_message_now(),
            Message::CopyRequestBody(request_config) => request_config,
        );
        assert_eq!(request_config, expected_config);
    }

    /// Test "Copy as cURL" action, which is available via the Recipe List or
    /// Recipe panes
    #[rstest]
    fn test_copy_as_curl(mut harness: TestHarness, terminal: TestTerminal) {
        let expected_config = RequestConfig {
            recipe_id: harness.collection.first_recipe_id().clone(),
            profile_id: Some(harness.collection.first_profile_id().clone()),
            options: BuildOptions::default(),
        };
        let mut component = create_component(&mut harness, &terminal);
        component
            .update_draw(Event::new_local(RecipeMenuAction::CopyCurl))
            .assert_empty();

        let request_config = assert_matches!(
            harness.pop_message_now(),
            Message::CopyRequestCurl(request_config) => request_config,
        );
        assert_eq!(request_config, expected_config);
    }
}
