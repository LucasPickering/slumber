//! Components for the "primary" view, which is the paned request/response view

use crate::{
    http::{RequestState, RequestStateType},
    message::{Message, RequestConfig},
    util::ResultReported,
    view::{
        Component, ViewContext,
        common::{
            actions::{IntoMenuAction, MenuAction},
            modal::Modal,
        },
        component::{
            exchange_pane::{ExchangePane, ExchangePaneEvent},
            help::HelpModal,
            misc::DeleteRecipeRequestsModal,
            profile_select::ProfilePane,
            recipe_list::{RecipeListPane, RecipeListPaneEvent},
            recipe_pane::{
                RecipeMenuAction, RecipePane, RecipePaneEvent, RecipePaneProps,
            },
        },
        context::UpdateContext,
        draw::{Draw, DrawMetadata},
        event::{Child, Emitter, Event, EventHandler, OptionEvent, ToEmitter},
        state::{
            StateCell,
            fixed_select::FixedSelectState,
            select::{SelectStateEvent, SelectStateEventType},
        },
        util::persistence::{Persisted, PersistedLazy},
    },
};
use derive_more::Display;
use persisted::SingletonKey;
use ratatui::{
    Frame,
    layout::Layout,
    prelude::{Constraint, Rect},
};
use serde::{Deserialize, Serialize};
use slumber_config::Action;
use slumber_core::{
    collection::{Collection, ProfileId, RecipeId, RecipeNodeType},
    http::RequestId,
};
use strum::{EnumCount, EnumIter, IntoEnumIterator};

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
    /// The exchange pane shows a particular request/response. The entire
    /// component is rebuilt whenever the selected request changes. The key is
    /// `None` if the recipe list is empty or a folder is selected
    exchange_pane: StateCell<
        Option<(RequestId, RequestStateType)>,
        Component<ExchangePane>,
    >,

    global_actions_emitter: Emitter<GlobalMenuAction>,
}

impl PrimaryView {
    pub fn new(collection: &Collection) -> Self {
        let profile_pane = ProfilePane::new(collection);
        let recipe_list_pane = RecipeListPane::new(&collection.recipes);

        Self {
            selected_pane: PersistedLazy::new(
                SingletonKey::default(),
                FixedSelectState::builder()
                    .subscribe([SelectStateEventType::Select])
                    .build(),
            ),
            fullscreen_mode: Default::default(),

            recipe_list_pane: recipe_list_pane.into(),
            profile_pane: profile_pane.into(),
            recipe_pane: Default::default(),
            exchange_pane: Default::default(),

            global_actions_emitter: Default::default(),
        }
    }

    /// Which recipe in the recipe list is selected? `None` iff the list is
    /// empty OR a folder is selected.
    pub fn selected_recipe_id(&self) -> Option<&RecipeId> {
        self.recipe_list_pane
            .data()
            .selected_node()
            .and_then(|(id, kind)| {
                if matches!(kind, RecipeNodeType::Recipe) {
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

    /// Get a definition of the request that should be sent from the current
    /// recipe settings
    pub fn request_config(&self) -> Option<RequestConfig> {
        self.recipe_pane.data().request_config()
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

    /// Get the current placement and focus for all panes, according to current
    /// selection and fullscreen state. We always draw all panes so they can
    /// perform their state updates. To hide them we just render to an empty
    /// rect.
    fn panes(&self, area: Rect) -> Panes {
        if let Some(fullscreen_mode) = *self.fullscreen_mode {
            match fullscreen_mode {
                FullscreenMode::Recipe => Panes {
                    profile: PaneState::default(),
                    recipe_list: PaneState::default(),
                    recipe: PaneState { area, focus: true },
                    exchange: PaneState::default(),
                },
                FullscreenMode::Exchange => Panes {
                    profile: PaneState::default(),
                    recipe_list: PaneState::default(),
                    recipe: PaneState::default(),
                    exchange: PaneState { area, focus: true },
                },
            }
        } else {
            // Split the main pane horizontally
            let [left_area, right_area] =
                Layout::horizontal([Constraint::Max(40), Constraint::Min(40)])
                    .areas(area);

            let [profile_area, recipe_list_area] =
                Layout::vertical([Constraint::Length(3), Constraint::Min(0)])
                    .areas(left_area);
            let [recipe_area, exchange_area] =
                self.get_right_column_layout(right_area);
            Panes {
                profile: PaneState {
                    area: profile_area,
                    focus: true,
                },
                recipe_list: PaneState {
                    area: recipe_list_area,
                    focus: self.is_selected(PrimaryPane::RecipeList),
                },
                recipe: PaneState {
                    area: recipe_area,
                    focus: self.is_selected(PrimaryPane::Recipe),
                },
                exchange: PaneState {
                    area: exchange_area,
                    focus: self.is_selected(PrimaryPane::Exchange),
                },
            }
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

    /// Send a request for the currently selected recipe
    fn send_request(&self) {
        ViewContext::send_message(Message::HttpBeginRequest);
    }

    /// Handle a menu action from the recipe list or recipe pane
    fn handle_recipe_menu_action(&self, action: RecipeMenuAction) {
        match action {
            RecipeMenuAction::CopyUrl => {
                ViewContext::send_message(Message::CopyRequestUrl)
            }
            RecipeMenuAction::CopyCurl => {
                ViewContext::send_message(Message::CopyRequestCurl)
            }
            RecipeMenuAction::DeleteRecipe => {
                if let Some(recipe_id) = self.selected_recipe_id() {
                    DeleteRecipeRequestsModal::new(
                        self.selected_profile_id().cloned(),
                        recipe_id.clone(),
                    )
                    .open()
                }
            }
        }
    }
}

impl EventHandler for PrimaryView {
    fn update(&mut self, _: &mut UpdateContext, event: Event) -> Option<Event> {
        event
            .opt()
            .action(|action, propagate| match action {
                Action::PreviousPane => self.selected_pane.get_mut().previous(),
                Action::NextPane => self.selected_pane.get_mut().next(),
                // Send a request from anywhere
                Action::Submit => self.send_request(),
                Action::OpenHelp => HelpModal.open(),

                // Pane hotkeys
                Action::SelectProfileList => {
                    self.profile_pane.data_mut().open_modal()
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
                _ => propagate.set(),
            })
            .emitted(self.selected_pane.to_emitter(), |event| {
                if let SelectStateEvent::Select(_) = event {
                    // Exit fullscreen when pane changes
                    self.maybe_exit_fullscreen();
                }
            })
            .emitted(self.recipe_list_pane.to_emitter(), |event| match event {
                RecipeListPaneEvent::Click => {
                    self.selected_pane
                        .get_mut()
                        .select(&PrimaryPane::RecipeList);
                }
                // Menu action forwarded up
                RecipeListPaneEvent::Action(action) => {
                    self.handle_recipe_menu_action(action)
                }
            })
            .emitted(self.recipe_pane.to_emitter(), |event| match event {
                RecipePaneEvent::Click => {
                    self.selected_pane.get_mut().select(&PrimaryPane::Recipe);
                }
                RecipePaneEvent::Action(action) => {
                    self.handle_recipe_menu_action(action)
                }
            })
            .emitted(self.exchange_pane.borrow().to_emitter(), |event| {
                match event {
                    ExchangePaneEvent::Click => self
                        .selected_pane
                        .get_mut()
                        .select(&PrimaryPane::Exchange),
                }
            })
            .emitted(self.global_actions_emitter, |menu_action| {
                // Handle our own menu action type
                match menu_action {
                    GlobalMenuAction::EditCollection => {
                        ViewContext::send_message(Message::CollectionEdit)
                    }
                }
            })
    }

    fn menu_actions(&self) -> Vec<MenuAction> {
        GlobalMenuAction::iter()
            .map(MenuAction::with_data(self, self.global_actions_emitter))
            .collect()
    }

    fn children(&mut self) -> Vec<Component<Child<'_>>> {
        vec![
            self.profile_pane.to_child_mut(),
            self.recipe_list_pane.to_child_mut(),
            self.recipe_pane.to_child_mut(),
            self.exchange_pane.get_mut().to_child_mut(),
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
        // We draw all panes regardless of fullscreen state, so they can run
        // their necessary state updates. We just give the hidden panes an empty
        // rect to draw into so they don't appear at all
        let panes = self.panes(metadata.area());

        self.profile_pane.draw(
            frame,
            (),
            panes.profile.area,
            panes.profile.focus,
        );
        self.recipe_list_pane.draw(
            frame,
            (),
            panes.recipe_list.area,
            panes.recipe_list.focus,
        );

        let collection = ViewContext::collection();
        let selected_recipe_node = self
            .recipe_list_pane
            .data()
            .selected_node()
            .and_then(|(id, _)| {
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
            panes.recipe.area,
            panes.recipe.focus,
        );

        // Rebuild the exchange pane whenever we select a new request or the
        // current request transitions between states
        let exchange_pane = self.exchange_pane.get_or_update(
            &props.selected_request.map(|request_state| {
                (request_state.id(), request_state.into())
            }),
            || {
                ExchangePane::new(
                    props.selected_request,
                    self.recipe_list_pane
                        .data()
                        .selected_node()
                        .map(|(_, node_type)| node_type),
                )
                .into()
            },
        );
        exchange_pane.draw(
            frame,
            (),
            panes.exchange.area,
            panes.exchange.focus,
        );
    }
}

#[derive(Clone, Debug, Default)]
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
enum PrimaryPane {
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

/// Menu actions available in all contexts
#[derive(Copy, Clone, Debug, Display, EnumIter)]
enum GlobalMenuAction {
    #[display("Edit Collection")]
    EditCollection,
}

impl IntoMenuAction<PrimaryView> for GlobalMenuAction {}

/// Helper for adjusting pane behavior according to state
struct Panes {
    profile: PaneState,
    recipe_list: PaneState,
    recipe: PaneState,
    exchange: PaneState,
}

/// Helper for adjusting pane behavior according to state
#[derive(Default)]
struct PaneState {
    area: Rect,
    focus: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        message::{Message, RequestConfig},
        test_util::{TestHarness, TestTerminal, harness, terminal},
        view::{
            test_util::TestComponent, util::persistence::DatabasePersistedStore,
        },
    };
    use crossterm::event::KeyCode;
    use persisted::PersistedStore;
    use rstest::rstest;
    use slumber_core::http::BuildOptions;
    use slumber_util::assert_matches;

    /// Create component to be tested
    fn create_component<'term>(
        harness: &mut TestHarness,
        terminal: &'term TestTerminal,
    ) -> TestComponent<'term, PrimaryView, PrimaryViewProps<'term>> {
        let view = PrimaryView::new(&harness.collection);
        let mut component = TestComponent::new(harness, terminal, view);
        // Initial events
        assert_matches!(
            component.int().drain_draw().events(),
            &[Event::HttpSelectRequest(None)]
        );
        // Clear template preview messages so we can test what we want
        harness.clear_messages();
        component
    }

    /// Test selected pane and fullscreen mode loading from persistence
    #[rstest]
    fn test_pane_persistence(mut harness: TestHarness, terminal: TestTerminal) {
        DatabasePersistedStore::store_persisted(
            &SingletonKey::<PrimaryPane>::default(),
            &PrimaryPane::Exchange,
        );
        DatabasePersistedStore::store_persisted(
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

    /// Test the request_config() getter
    #[rstest]
    fn test_request_config(mut harness: TestHarness, terminal: TestTerminal) {
        let component = create_component(&mut harness, &terminal);
        let expected_config = RequestConfig {
            recipe_id: harness.collection.first_recipe_id().clone(),
            profile_id: Some(harness.collection.first_profile_id().clone()),
            options: BuildOptions::default(),
        };
        assert_eq!(component.data().request_config(), Some(expected_config));
    }

    /// Test "Edit Collection" action
    #[rstest]
    fn test_edit_collection(mut harness: TestHarness, terminal: TestTerminal) {
        let mut component = create_component(&mut harness, &terminal);
        component.int().drain_draw().assert_empty();

        harness.clear_messages(); // Clear init junk

        component
            .int()
            .open_actions()
            .send_key(KeyCode::Enter) // Select first action - Edit Collection
            .assert_empty();
        // Event should be converted into a message appropriately
        assert_matches!(harness.pop_message_now(), Message::CollectionEdit);
    }

    /// Test "Copy URL" action, which is available via the Recipe List or Recipe
    /// panes
    #[rstest]
    fn test_copy_url(mut harness: TestHarness, terminal: TestTerminal) {
        let mut component = create_component(&mut harness, &terminal);

        component
            .int()
            .send_key(KeyCode::Char('l')) // Select recipe list
            .open_actions()
            // Copy URL
            .send_keys([KeyCode::Down, KeyCode::Enter])
            .assert_empty();

        assert_matches!(harness.pop_message_now(), Message::CopyRequestUrl);
    }

    /// Test "Copy as cURL" action, which is available via the Recipe List or
    /// Recipe panes
    #[rstest]
    fn test_copy_as_curl(mut harness: TestHarness, terminal: TestTerminal) {
        let mut component = create_component(&mut harness, &terminal);

        component
            .int()
            .send_key(KeyCode::Char('l')) // Select recipe list
            .open_actions()
            // Copy as cURL
            .send_keys([KeyCode::Down, KeyCode::Down, KeyCode::Enter])
            .assert_empty();

        assert_matches!(harness.pop_message_now(), Message::CopyRequestCurl);
    }
}
