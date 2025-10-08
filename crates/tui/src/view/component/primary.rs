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
            collection_select::CollectionSelect,
            exchange_pane::{ExchangePane, ExchangePaneEvent},
            help::HelpModal,
            misc::DeleteRecipeRequestsModal,
            profile_list::{
                ProfileDetail, ProfileDetailProps, ProfileTab, ProfileTabEvent,
            },
            recipe_list::{RecipeListPane, RecipeTabEvent},
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
use ratatui::{
    Frame,
    layout::Layout,
    prelude::{Constraint, Rect},
};
use serde::{Deserialize, Serialize};
use slumber_config::Action;
use slumber_core::{
    collection::{Collection, ProfileId, RecipeId, RecipeNode, RecipeNodeType},
    http::RequestId,
};
use strum::{EnumCount, EnumIter, IntoEnumIterator};

/// Primary TUI view, which shows request/response panes
#[derive(Debug)]
pub struct PrimaryView {
    // Own state
    selected_pane: PersistedLazy<PrimaryPaneKey, FixedSelectState<PrimaryPane>>,
    /// Which sidebar tab (if any) is open?
    open_tab: Persisted<SidebarTabKey>,
    fullscreen_pane: Persisted<FullscreenPaneKey>,

    // Children
    // TODO consistent naming for these fields/components
    profile_tab: Component<ProfileTab>,
    /// Ephemeral pane to show the fields of a profile. Replaces the
    /// Request/Response pane when the Profile list is open
    profile_detail: Component<ProfileDetail>,
    recipe_tab: Component<RecipeListPane>,
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
        let profile_pane = ProfileTab::new(collection);
        let recipe_list_pane = RecipeListPane::new(&collection.recipes);

        Self {
            selected_pane: PersistedLazy::new(
                Default::default(),
                FixedSelectState::builder()
                    .subscribe([SelectStateEventType::Select])
                    .build(),
            ),
            open_tab: Persisted::default(),
            fullscreen_pane: Default::default(),

            profile_tab: profile_pane.into(),
            profile_detail: Default::default(),
            recipe_tab: recipe_list_pane.into(),
            recipe_pane: Default::default(),
            exchange_pane: Default::default(),

            global_actions_emitter: Default::default(),
        }
    }

    /// Which recipe in the recipe list is selected? `None` iff the list is
    /// empty OR a folder is selected.
    pub fn selected_recipe_id(&self) -> Option<&RecipeId> {
        self.recipe_tab
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
        self.profile_tab.data().selected_profile_id()
    }

    /// Get a definition of the request that should be sent from the current
    /// recipe settings
    pub fn request_config(&self) -> Option<RequestConfig> {
        self.recipe_pane.data().request_config()
    }

    /// Is the given pane selected?
    fn is_selected(&self, pane: PrimaryPane) -> bool {
        self.selected_pane.is_selected(&pane)
    }

    /// Select a particular pane
    fn select_pane(&mut self, pane: PrimaryPane) {
        self.selected_pane.get_mut().select(&pane);
    }

    fn is_sidebar_open(&self) -> bool {
        self.open_tab.is_some()
    }

    /// Select a sidebar tab
    fn open_sidebar(&mut self, tab: SidebarTab) {
        *self.open_tab.get_mut() = Some(tab);
    }

    /// Close the sidebar tab
    fn close_sidebar(&mut self) {
        *self.open_tab.get_mut() = None;
    }

    /// Select the next or previous pane. If the sidebar is open, just close it
    /// instead of cycling
    fn cycle_pane(&mut self, next: bool) {
        if self.is_sidebar_open() {
            self.close_sidebar();
        } else {
            let mut pane = self.selected_pane.get_mut();
            if next {
                pane.next();
            } else {
                pane.previous();
            }
        }
    }

    /// Toggle fullscreen on the selected pane
    fn toggle_fullscreen(&mut self) {
        // The sidebar can't be fullscreened, so if it's open do nothing
        if self.is_sidebar_open() {
            return;
        }

        let selected_pane = self.selected_pane.selected();
        // If we're already in the given mode, exit
        let is_fullscreened = *self.fullscreen_pane == Some(selected_pane);
        *self.fullscreen_pane.get_mut() = if is_fullscreened {
            None
        } else {
            Some(selected_pane)
        };
    }

    /// Exit fullscreen mode if it doesn't match the selected pane. This is
    /// called when the pane changes, but it's possible they match when we're
    /// loading from persistence. In those cases, stay in fullscreen.
    fn maybe_exit_fullscreen(&mut self) {
        match (self.selected_pane.selected(), *self.fullscreen_pane) {
            (PrimaryPane::Recipe, Some(PrimaryPane::Recipe))
            | (PrimaryPane::Exchange, Some(PrimaryPane::Exchange)) => {}
            _ => *self.fullscreen_pane.get_mut() = None,
        }
    }

    /// Get the current placement and focus for all panes, according to current
    /// selection and fullscreen state. We always draw all panes so they can
    /// perform their state updates. To hide them we just render to an empty
    /// rect.
    fn panes(&self, area: Rect) -> Panes {
        if let Some(fullscreen_mode) = *self.fullscreen_pane {
            match fullscreen_mode {
                PrimaryPane::Recipe => Panes {
                    profile: PaneState::default(),
                    recipe_list: PaneState::default(),
                    recipe: PaneState { area, focus: true },
                    exchange: PaneState::default(),
                },
                PrimaryPane::Exchange => Panes {
                    profile: PaneState::default(),
                    recipe_list: PaneState::default(),
                    recipe: PaneState::default(),
                    exchange: PaneState { area, focus: true },
                },
            }
        } else if let Some(tab) = *self.open_tab {
            // If one of the list tabs is selected, open the sidebar
            // TODO draw a diagram
            let [sidebar_area, main_area] = Layout::horizontal([
                Constraint::Length(40),
                Constraint::Min(0),
            ])
            .areas(area);
            let [top_area, recipe_area, exchange_area] = Layout::vertical([
                Constraint::Length(3),
                Constraint::Ratio(1, 2),
                Constraint::Ratio(1, 2),
            ])
            .areas(main_area);
            let [profile_area, recipe_list_area] = Layout::horizontal([
                Constraint::Ratio(1, 2),
                Constraint::Ratio(1, 2),
            ])
            .areas(top_area);

            // TODO simplify this code
            Panes {
                profile: PaneState {
                    area: if tab == SidebarTab::Profile {
                        sidebar_area
                    } else {
                        profile_area
                    },
                    focus: tab == SidebarTab::Profile,
                },
                recipe_list: PaneState {
                    area: if tab == SidebarTab::Recipe {
                        sidebar_area
                    } else {
                        recipe_list_area
                    },
                    focus: tab == SidebarTab::Recipe,
                },
                recipe: PaneState {
                    area: recipe_area,
                    focus: false,
                },
                exchange: PaneState {
                    area: exchange_area,
                    focus: false,
                },
            }
        } else {
            // Sidebar is closed. We'll show the tabs at the top but the user is
            // in one of the primary panes
            let [top_area, recipe_area, exchange_area] = Layout::vertical([
                Constraint::Length(3),
                Constraint::Ratio(1, 2),
                Constraint::Ratio(1, 2),
            ])
            .areas(area);
            let [profile_area, recipe_list_area] = Layout::horizontal([
                Constraint::Ratio(1, 2),
                Constraint::Ratio(1, 2),
            ])
            .areas(top_area);

            Panes {
                profile: PaneState {
                    area: profile_area,
                    focus: false,
                },
                recipe_list: PaneState {
                    area: recipe_list_area,
                    focus: false,
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

    /// Send a request for the currently selected recipe
    fn send_request(&self) {
        ViewContext::send_message(Message::HttpBeginRequest);
    }

    /// Handle a menu action from the recipe list or recipe pane
    fn handle_recipe_menu_action(&self, action: RecipeMenuAction) {
        match action {
            RecipeMenuAction::CopyUrl => {
                ViewContext::send_message(Message::CopyRequestUrl);
            }
            RecipeMenuAction::CopyCurl => {
                ViewContext::send_message(Message::CopyRequestCurl);
            }
            RecipeMenuAction::DeleteRecipe => {
                if let Some(recipe_id) = self.selected_recipe_id() {
                    DeleteRecipeRequestsModal::new(
                        self.selected_profile_id().cloned(),
                        recipe_id.clone(),
                    )
                    .open();
                }
            }
        }
    }

    /// Send a message to open the collection file to the selected
    /// recipe/folder. If the collection is empty, just open to the start
    fn edit_selected_recipe(&self) {
        let collection = ViewContext::collection();
        // Get the source location of the selected folder/recipe
        let location = self
            .recipe_tab
            .data()
            .selected_node()
            .and_then(|(id, _)| collection.recipes.get(id))
            .map(RecipeNode::location)
            .cloned();
        ViewContext::send_message(Message::CollectionEdit { location });
    }
}

impl EventHandler for PrimaryView {
    fn update(&mut self, _: &mut UpdateContext, event: Event) -> Option<Event> {
        event
            .opt()
            .action(|action, propagate| match action {
                Action::PreviousPane => self.cycle_pane(false),
                Action::NextPane => self.cycle_pane(true),
                // Send a request from anywhere
                Action::Submit => self.send_request(),
                Action::OpenHelp => HelpModal.open(),

                // Pane hotkeys
                Action::SelectProfileList => {
                    self.open_sidebar(SidebarTab::Profile);
                }
                Action::SelectRecipeList => {
                    self.open_sidebar(SidebarTab::Recipe);
                }
                Action::SelectRecipe => {
                    self.select_pane(PrimaryPane::Recipe);
                }
                Action::SelectResponse => {
                    self.select_pane(PrimaryPane::Exchange);
                }
                Action::SelectCollection => CollectionSelect::new().open(),

                // Toggle fullscreen
                Action::Fullscreen => self.toggle_fullscreen(),
                // Exit fullscreen
                Action::Cancel if self.fullscreen_pane.is_some() => {
                    *self.fullscreen_pane.get_mut() = None;
                }
                _ => propagate.set(),
            })
            .emitted(self.selected_pane.to_emitter(), |event| {
                if let SelectStateEvent::Select(_) = event {
                    // Exit fullscreen when pane changes
                    self.maybe_exit_fullscreen();
                }
            })
            .emitted(self.profile_tab.to_emitter(), |event| match event {
                ProfileTabEvent::Click => {
                    self.open_sidebar(SidebarTab::Profile);
                }
                ProfileTabEvent::Submit => {
                    // We have a new profile
                    self.close_sidebar();
                    // Refresh template previews
                    ViewContext::push_event(Event::HttpSelectRequest(None));
                }
            })
            .emitted(self.recipe_tab.to_emitter(), |event| match event {
                RecipeTabEvent::Click => self.open_sidebar(SidebarTab::Recipe),
                RecipeTabEvent::Submit => self.close_sidebar(),
                // Menu action forwarded up
                RecipeTabEvent::Action(action) => {
                    self.handle_recipe_menu_action(action);
                }
            })
            .emitted(self.recipe_pane.to_emitter(), |event| match event {
                RecipePaneEvent::Click => {
                    self.select_pane(PrimaryPane::Recipe);
                }
                RecipePaneEvent::Action(action) => {
                    self.handle_recipe_menu_action(action);
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
            // Handle our own menu action type
            .emitted(self.global_actions_emitter, |menu_action| {
                match menu_action {
                    GlobalMenuAction::EditRecipe => self.edit_selected_recipe(),
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
            self.profile_tab.to_child_mut(),
            self.recipe_tab.to_child_mut(),
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

        self.profile_tab.draw(
            frame,
            (),
            panes.profile.area,
            panes.profile.focus,
        );
        self.recipe_tab.draw(
            frame,
            (),
            panes.recipe_list.area,
            panes.recipe_list.focus,
        );

        let collection = ViewContext::collection();
        let selected_recipe_node =
            self.recipe_tab.data().selected_node().and_then(|(id, _)| {
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
        if let Some(profile_id) = self.profile_tab.data().selected_profile_id()
            && *self.open_tab == Some(SidebarTab::Profile)
        {
            // If the profile list is open, render the profile detail instead of
            // the request/response
            self.profile_detail.draw(
                frame,
                ProfileDetailProps { profile_id },
                panes.exchange.area,
                false,
            );
        } else {
            // Rebuild the exchange pane whenever we select a new request or the
            // current request transitions between states
            let exchange_pane = self.exchange_pane.get_or_update(
                &props.selected_request.map(|request_state| {
                    (request_state.id(), request_state.into())
                }),
                || {
                    ExchangePane::new(
                        props.selected_request,
                        self.recipe_tab
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
}

#[derive(Debug, Default)]
pub struct PrimaryViewProps<'a> {
    pub selected_request: Option<&'a RequestState>,
}

/// Persistence key for selected pane
#[derive(Debug, Default, persisted::PersistedKey, Serialize)]
#[persisted(PrimaryPane)]
struct PrimaryPaneKey;

/// Selectable and fullscreenable panes
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
    Recipe,
    Exchange,
}

/// Persistence key for opened sidebar tab
#[derive(Debug, Default, persisted::PersistedKey, Serialize)]
#[persisted(Option<SidebarTab>)]
struct SidebarTabKey;

/// Tabs at the top of the screen that can be opened in the sidebar
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
enum SidebarTab {
    Profile,
    Recipe,
}

/// Persistence key for fullscreen mode
#[derive(Debug, Default, persisted::PersistedKey, Serialize)]
#[persisted(Option<PrimaryPane>)]
struct FullscreenPaneKey;

/// Menu actions available in all contexts
#[derive(Copy, Clone, Debug, Display, EnumIter)]
enum GlobalMenuAction {
    /// Open the collection file in an external editor, jumping to whatever
    /// recipe is currently selected
    #[display("Edit Recipe")]
    EditRecipe,
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
    use persisted::PersistedStore;
    use rstest::rstest;
    use slumber_core::http::BuildOptions;
    use slumber_util::assert_matches;
    use terminput::KeyCode;

    /// Create component to be tested
    fn create_component<'term>(
        harness: &mut TestHarness,
        terminal: &'term TestTerminal,
    ) -> TestComponent<'term, PrimaryView> {
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
            &PrimaryPaneKey,
            &PrimaryPane::Exchange,
        );
        DatabasePersistedStore::store_persisted(
            &FullscreenPaneKey,
            &Some(PrimaryPane::Exchange),
        );

        let component = create_component(&mut harness, &terminal);
        assert_eq!(
            component.data().selected_pane.selected(),
            PrimaryPane::Exchange
        );
        assert_matches!(
            *component.data().fullscreen_pane,
            Some(PrimaryPane::Exchange)
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

    /// Test "Edit Recipe" action
    #[rstest]
    fn test_edit_recipe(mut harness: TestHarness, terminal: TestTerminal) {
        let mut component = create_component(&mut harness, &terminal);
        component.int().drain_draw().assert_empty();

        harness.clear_messages(); // Clear init junk

        component.int().action("Edit Recipe").assert_empty();
        // Event should be converted into a message appropriately
        assert_matches!(
            harness.pop_message_now(),
            // The actual location is unimportant because the collection was
            // generated in memory, but make sure it's populated
            Message::CollectionEdit { location: Some(_) }
        );
    }

    /// Test "Copy URL" action, which is available via the Recipe List or Recipe
    /// panes
    #[rstest]
    fn test_copy_url(mut harness: TestHarness, terminal: TestTerminal) {
        let mut component = create_component(&mut harness, &terminal);

        component
            .int()
            .send_key(KeyCode::Char('l')) // Select recipe list
            .action("Copy URL")
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
            .action("Copy as cURL")
            .assert_empty();

        assert_matches!(harness.pop_message_now(), Message::CopyRequestCurl);
    }
}
