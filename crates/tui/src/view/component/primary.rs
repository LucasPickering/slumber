//! Components for the "primary" view, which is the paned request/response view

use crate::{
    http::{RequestConfig, RequestState, RequestStateType},
    message::{Message, RecipeCopyTarget},
    util::{PersistentKey, PersistentStore, ResultReported},
    view::{
        Component, ViewContext,
        common::{actions::MenuItem, modal::ModalQueue},
        component::{
            Canvas, Child, ComponentId, Draw, DrawMetadata, ToChild,
            collection_select::CollectionSelect,
            exchange_pane::ExchangePane,
            misc::DeleteRecipeRequestsModal,
            profile::{
                ProfileListItem, ProfileListState, ProfilePreview,
                ProfilePreviewProps,
            },
            recipe_list::RecipeListState,
            recipe_pane::{
                RecipeMenuAction, RecipePane, RecipePaneEvent, RecipePaneProps,
            },
            sidebar_list::{PrimaryListEvent, SidebarList, SidebarListProps},
        },
        context::UpdateContext,
        event::{Emitter, Event, EventMatch, ToEmitter},
        state::StateCell,
    },
};
use ratatui::{
    layout::{Layout, Spacing},
    prelude::{Constraint, Rect},
};
use serde::{Deserialize, Serialize};
use slumber_config::Action;
use slumber_core::{
    collection::{HasId, ProfileId, RecipeId, RecipeNode, RecipeNodeType},
    http::RequestId,
};
use strum::{EnumIter, IntoEnumIterator};

/// Primary TUI view, which shows request/response panes
#[derive(Debug)]
pub struct PrimaryView {
    id: ComponentId,
    // Own state
    /// Current layout and selection state of the view
    view: ViewState,

    // Children
    /// Header/sidebar to select a recipe
    recipe_list: SidebarList<RecipeListState>,
    /// Recipe preview/detail pane
    /// TODO rename
    recipe_pane: RecipePane,
    /// Header/sidebar to select a profile
    profile_list: SidebarList<ProfileListState>,
    /// Profile preview/detail pane
    /// TODO rename
    profile_pane: ProfilePreview,
    /// The exchange pane shows a particular request/response. The entire
    /// component is rebuilt whenever the selected request changes. The key is
    /// `None` if the recipe list is empty or a folder is selected
    exchange_pane:
        StateCell<Option<(RequestId, RequestStateType)>, ExchangePane>,
    /// Modal to select a different collection file
    collection_select: ModalQueue<CollectionSelect>,
    /// Modal to delete all requests for a recipe
    delete_requests_modal: ModalQueue<DeleteRecipeRequestsModal>,

    global_actions_emitter: Emitter<PrimaryMenuAction>,
}

impl PrimaryView {
    pub fn new() -> Self {
        let view = PersistentStore::get(&ViewStateKey).unwrap_or_default();
        Self {
            id: ComponentId::default(),
            view,

            recipe_list: SidebarList::default(),
            recipe_pane: RecipePane::default(),
            profile_list: SidebarList::default(),
            profile_pane: ProfilePreview::default(),
            exchange_pane: Default::default(),
            collection_select: Default::default(),
            delete_requests_modal: Default::default(),

            global_actions_emitter: Default::default(),
        }
    }

    /// Which recipe in the recipe list is selected? `None` iff the list is
    /// empty OR a folder is selected.
    pub fn selected_recipe_id(&self) -> Option<&RecipeId> {
        self.selected_recipe_node().and_then(|(id, kind)| {
            if matches!(kind, RecipeNodeType::Recipe) {
                Some(id)
            } else {
                None
            }
        })
    }

    /// ID of the selected profile. `None` iff the list is empty
    pub fn selected_profile_id(&self) -> Option<&ProfileId> {
        self.profile_list.selected().map(ProfileListItem::id)
    }

    fn selected_recipe_node(&self) -> Option<(&RecipeId, RecipeNodeType)> {
        self.recipe_list
            .selected()
            .map(|item| (item.id(), item.kind()))
    }

    /// Get a definition of the request that should be sent from the current
    /// recipe settings
    pub fn request_config(&self) -> Option<RequestConfig> {
        self.recipe_pane.request_config()
    }

    /// Send a request for the currently selected recipe
    fn send_request(&self) {
        ViewContext::send_message(Message::HttpBeginRequest);
    }

    /// Handle a menu action from the recipe list or recipe pane
    fn handle_recipe_menu_action(&mut self, action: RecipeMenuAction) {
        match action {
            RecipeMenuAction::CopyUrl => ViewContext::send_message(
                Message::CopyRecipe(RecipeCopyTarget::Url),
            ),
            RecipeMenuAction::CopyAsCli => ViewContext::send_message(
                Message::CopyRecipe(RecipeCopyTarget::Cli),
            ),
            RecipeMenuAction::CopyAsCurl => ViewContext::send_message(
                Message::CopyRecipe(RecipeCopyTarget::Curl),
            ),
            RecipeMenuAction::CopyAsPython => ViewContext::send_message(
                Message::CopyRecipe(RecipeCopyTarget::Python),
            ),
            RecipeMenuAction::DeleteRecipe => {
                if let Some(recipe_id) = self.selected_recipe_id() {
                    self.delete_requests_modal.open(
                        DeleteRecipeRequestsModal::new(
                            self.selected_profile_id().cloned(),
                            recipe_id.clone(),
                        ),
                    );
                }
            }
        }
    }

    /// Send a message to open the collection file to the selected
    /// recipe/folder. If there are no recipes, just open to the start
    fn edit_collection(&self) {
        let collection = ViewContext::collection();
        // Get the source location of the selected folder/recipe
        let location = self
            .selected_recipe_node()
            .and_then(|(id, _)| collection.recipes.get(id))
            .map(RecipeNode::location)
            .cloned();
        ViewContext::send_message(Message::CollectionEdit { location });
    }
}

impl Component for PrimaryView {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn update(&mut self, _: &mut UpdateContext, event: Event) -> EventMatch {
        // TODO exit fullscreen whenever selected pane changes
        event
            .m()
            .click(|position, _| {
                if self.recipe_pane.contains(position) {
                    self.view.select_recipe_pane();
                } else if self.profile_pane.contains(position) {
                    self.view.select_profile_pane();
                } else if self.exchange_pane.get_mut().contains(position) {
                    self.view.select_exchange_pane();
                }
            })
            .action(|action, propagate| match action {
                Action::PreviousPane => self.view.previous_pane(),
                Action::NextPane => self.view.next_pane(),
                // Send a request from anywhere
                Action::Submit => self.send_request(),

                // Pane hotkeys
                Action::SelectProfileList => self.view.open_profile_list(),
                Action::SelectProfile => self.view.select_profile_pane(),
                Action::SelectRecipeList => self.view.open_recipe_list(),
                Action::SelectRecipe => self.view.select_recipe_pane(),
                Action::SelectResponse => self.view.select_exchange_pane(),
                Action::SelectCollection => {
                    self.collection_select.open(CollectionSelect::new());
                }

                // Toggle fullscreen
                Action::Fullscreen => self.view.toggle_fullscreen(),
                // Exit fullscreen
                // TODO fix fullscreen
                // Action::Cancel if self.fullscreen.is_some() => {
                //     self.fullscreen = None;
                // }
                _ => propagate.set(),
            })
            .emitted(self.recipe_list.to_emitter(), |event| match event {
                PrimaryListEvent::Open => self.view.open_recipe_list(),
                PrimaryListEvent::Close => self.view.close_sidebar(),
            })
            .emitted(self.recipe_pane.to_emitter(), |event| match event {
                RecipePaneEvent::Action(action) => {
                    self.handle_recipe_menu_action(action);
                }
            })
            .emitted(self.profile_list.to_emitter(), |event| match event {
                PrimaryListEvent::Open => self.view.open_profile_list(),
                PrimaryListEvent::Close => self.view.close_sidebar(),
            })
            // Handle our own menu action type
            .emitted(self.global_actions_emitter, |menu_action| {
                match menu_action {
                    PrimaryMenuAction::EditCollection => {
                        self.edit_collection();
                    }
                }
            })
    }

    fn menu(&self) -> Vec<MenuItem> {
        // TODO edit profile too
        let emitter = self.global_actions_emitter;
        let edit_collection_name = match self.selected_recipe_node() {
            None => "Edit Collection",
            Some((_, RecipeNodeType::Folder)) => "Edit Folder",
            Some((_, RecipeNodeType::Recipe)) => "Edit Recipe",
        };
        vec![
            emitter
                .menu(PrimaryMenuAction::EditCollection, edit_collection_name)
                .into(),
        ]
    }

    fn persist(&self, store: &mut PersistentStore) {
        store.set(&ViewStateKey, &self.view);
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        vec![
            // Modals
            self.delete_requests_modal.to_child_mut(),
            self.collection_select.to_child_mut(),
            // Not modals
            self.recipe_list.to_child_mut(),
            self.recipe_pane.to_child_mut(),
            self.profile_list.to_child_mut(),
            self.profile_pane.to_child_mut(),
            self.exchange_pane.get_mut().to_child_mut(),
        ]
    }
}

impl<'a> Draw<PrimaryViewProps<'a>> for PrimaryView {
    fn draw(
        &self,
        canvas: &mut Canvas,
        props: PrimaryViewProps<'a>,
        metadata: DrawMetadata,
    ) {
        // Precompute recipe pane
        let collection = ViewContext::collection();
        let selected_recipe_node =
            self.selected_recipe_node().and_then(|(id, _)| {
                collection
                    .recipes
                    .try_get(id)
                    .reported(&ViewContext::messages_tx())
            });
        let recipe_props = RecipePaneProps {
            selected_recipe_node,
            selected_profile_id: self.selected_profile_id(),
        };

        // Precompute exchange paner
        // Rebuild the pane whenever we select a new request or the
        // current request transitions between states
        let exchange_pane = self.exchange_pane.get_or_update(
            &props.selected_request.map(|request_state| {
                (request_state.id(), request_state.into())
            }),
            || {
                ExchangePane::new(
                    props.selected_request,
                    self.selected_recipe_node().map(|(_, node_type)| node_type),
                )
            },
        );

        let area = metadata.area();
        match &self.view {
            // Sidebar is closed
            ViewState::Default {
                selected_pane,
                fullscreen: false,
            } => {
                let areas = DefaultAreas::new(area);

                // Header
                let [profile_list_area, recipe_list_area] = areas.headers;
                canvas.draw(
                    &self.profile_list,
                    SidebarListProps::header(),
                    profile_list_area,
                    false,
                );
                canvas.draw(
                    &self.recipe_list,
                    SidebarListProps::header(),
                    recipe_list_area,
                    false,
                );

                // Panes
                canvas.draw(
                    &self.recipe_pane,
                    recipe_props,
                    areas.top_pane,
                    *selected_pane == DefaultPane::Recipe,
                );
                canvas.draw(
                    &*exchange_pane,
                    (),
                    areas.bottom_pane,
                    *selected_pane == DefaultPane::Exchange,
                );
            }
            ViewState::Default {
                selected_pane,
                fullscreen: true,
            } => match selected_pane {
                DefaultPane::Recipe => {
                    canvas.draw(&self.recipe_pane, recipe_props, area, true);
                }
                DefaultPane::Exchange => {
                    canvas.draw(&*exchange_pane, (), area, true);
                }
            },

            // Profile list is open in sidebar
            ViewState::Profile {
                selected_pane,
                fullscreen: false,
            } => {
                let areas = SidebarAreas::new(area);

                // Header
                let [recipe_list_area] = areas.headers;
                canvas.draw(
                    &self.recipe_list,
                    SidebarListProps::header(),
                    recipe_list_area,
                    false,
                );

                // Sidebar
                canvas.draw(
                    &self.profile_list,
                    SidebarListProps::list(),
                    areas.sidebar,
                    *selected_pane == ProfileSelectPane::List,
                );

                // Panes
                canvas.draw(
                    &self.recipe_pane,
                    recipe_props,
                    areas.top_pane,
                    *selected_pane == ProfileSelectPane::Recipe,
                );
                canvas.draw(
                    &self.profile_pane,
                    ProfilePreviewProps {
                        profile_id: self.selected_profile_id(),
                    },
                    areas.bottom_pane,
                    *selected_pane == ProfileSelectPane::Profile,
                );
            }
            ViewState::Profile {
                selected_pane,
                fullscreen: true,
            } => match selected_pane {
                ProfileSelectPane::List => canvas.draw(
                    &self.profile_list,
                    SidebarListProps::list(),
                    area,
                    true,
                ),
                ProfileSelectPane::Recipe => {
                    canvas.draw(&self.recipe_pane, recipe_props, area, true);
                }
                ProfileSelectPane::Profile => canvas.draw(
                    &self.profile_pane,
                    ProfilePreviewProps {
                        profile_id: self.selected_profile_id(),
                    },
                    area,
                    true,
                ),
            },

            // Recipe list is open in sidebar
            ViewState::Recipe {
                selected_pane,
                fullscreen: false,
            } => {
                let areas = SidebarAreas::new(area);

                // Header
                let [profile_list_area] = areas.headers;
                canvas.draw(
                    &self.profile_list,
                    SidebarListProps::header(),
                    profile_list_area,
                    false,
                );

                // Sidebar
                canvas.draw(
                    &self.recipe_list,
                    SidebarListProps::list(),
                    areas.sidebar,
                    *selected_pane == RecipeSelectPane::List,
                );

                // Panes
                canvas.draw(
                    &self.recipe_pane,
                    recipe_props,
                    areas.top_pane,
                    *selected_pane == RecipeSelectPane::Recipe,
                );
                canvas.draw(
                    &*exchange_pane,
                    (),
                    areas.bottom_pane,
                    *selected_pane == RecipeSelectPane::Exchange,
                );
            }
            ViewState::Recipe {
                selected_pane,
                fullscreen: true,
            } => match selected_pane {
                RecipeSelectPane::List => canvas.draw(
                    &self.recipe_list,
                    SidebarListProps::list(),
                    area,
                    true,
                ),
                RecipeSelectPane::Recipe => {
                    canvas.draw(&self.recipe_pane, recipe_props, area, true);
                }
                RecipeSelectPane::Exchange => {
                    canvas.draw(&*exchange_pane, (), area, true);
                }
            },
        }

        // Modals!!
        canvas.draw_portal(&self.delete_requests_modal, (), true);
        canvas.draw_portal(&self.collection_select, (), true);
    }
}

#[derive(Debug, Default)]
pub struct PrimaryViewProps<'a> {
    pub selected_request: Option<&'a RequestState>,
}

/// TODO
#[derive(Debug, Serialize, Deserialize)]
enum ViewState {
    /// TODO
    Default {
        selected_pane: DefaultPane,
        fullscreen: bool,
    },
    /// TODO
    Profile {
        selected_pane: ProfileSelectPane,
        fullscreen: bool,
    },
    /// TODO
    Recipe {
        selected_pane: RecipeSelectPane,
        fullscreen: bool,
    },
}

impl ViewState {
    // TODO switching panes should exit fullscreen

    /// Open the profile list in the sidebar
    fn open_profile_list(&mut self) {
        *self = Self::Profile {
            selected_pane: ProfileSelectPane::List,
            fullscreen: false,
        };
    }

    /// Open the recipe list in the sidebar
    fn open_recipe_list(&mut self) {
        *self = Self::Recipe {
            selected_pane: RecipeSelectPane::List,
            fullscreen: false,
        };
    }

    /// Close the sidebar and return to the default view
    fn close_sidebar(&mut self) {
        // TODO retain selected pane if possible
        *self = Self::Default {
            selected_pane: DefaultPane::Recipe,
            fullscreen: false,
        };
    }

    /// Select the previous pane in the cycle
    fn previous_pane(&mut self) {
        fn previous<T: PartialEq + IntoEnumIterator>(value: T) -> T {
            T::iter()
                .rev() // Reverse to get previous!
                .cycle()
                .skip_while(|v| *v != value)
                .nth(1) // Get one *after* the found value
                .expect("Iterator is cycled so it always returns")
        }

        // Each state has a different pane type
        match self {
            ViewState::Default { selected_pane, .. } => {
                *selected_pane = previous(*selected_pane);
            }
            ViewState::Profile { selected_pane, .. } => {
                *selected_pane = previous(*selected_pane);
            }
            ViewState::Recipe { selected_pane, .. } => {
                *selected_pane = previous(*selected_pane);
            }
        }
    }

    /// Select the next pane in the cycle
    fn next_pane(&mut self) {
        fn next<T: PartialEq + IntoEnumIterator>(value: T) -> T {
            T::iter()
                .cycle()
                .skip_while(|v| *v != value)
                .nth(1) // Get one *after* the found value
                .expect("Iterator is cycled so it always returns")
        }

        // Each state has a different pane type
        match self {
            ViewState::Default { selected_pane, .. } => {
                *selected_pane = next(*selected_pane);
            }
            ViewState::Profile { selected_pane, .. } => {
                *selected_pane = next(*selected_pane);
            }
            ViewState::Recipe { selected_pane, .. } => {
                *selected_pane = next(*selected_pane);
            }
        }
    }

    /// TODO
    fn select_recipe_pane(&mut self) {
        match self {
            ViewState::Default { selected_pane, .. } => {
                *selected_pane = DefaultPane::Recipe;
            }
            ViewState::Profile { selected_pane, .. } => {
                *selected_pane = ProfileSelectPane::Recipe;
            }
            ViewState::Recipe { selected_pane, .. } => {
                *selected_pane = RecipeSelectPane::Recipe;
            }
        }
    }

    /// TODO
    fn select_profile_pane(&mut self) {
        match self {
            ViewState::Profile { selected_pane, .. } => {
                *selected_pane = ProfileSelectPane::Profile;
            }
            // Profile pane isn't visible
            ViewState::Default { .. } | ViewState::Recipe { .. } => {}
        }
    }

    /// TODO
    fn select_exchange_pane(&mut self) {
        match self {
            ViewState::Default { selected_pane, .. } => {
                *selected_pane = DefaultPane::Exchange;
            }
            ViewState::Profile { .. } => {} // Exchange pane isn't visible
            ViewState::Recipe { selected_pane, .. } => {
                *selected_pane = RecipeSelectPane::Exchange;
            }
        }
    }

    /// Enter/exit fullscreen mode for the currently selected pane
    fn toggle_fullscreen(&mut self) {
        match self {
            ViewState::Default { fullscreen, .. }
            | ViewState::Profile { fullscreen, .. }
            | ViewState::Recipe { fullscreen, .. } => *fullscreen ^= true,
        }
    }
}

impl Default for ViewState {
    fn default() -> Self {
        Self::Default {
            selected_pane: DefaultPane::Recipe,
            fullscreen: false,
        }
    }
}

/// Selectable pane in [ViewState::Default]
#[derive(Copy, Clone, Debug, PartialEq, EnumIter, Serialize, Deserialize)]
enum DefaultPane {
    Recipe,
    Exchange,
}

/// Selectable pane in [ViewState::Profile]
#[derive(Copy, Clone, Debug, PartialEq, EnumIter, Serialize, Deserialize)]
enum ProfileSelectPane {
    List,
    Recipe,
    Profile,
}

/// Selectable pane in [ViewState::Recipe]
#[derive(Copy, Clone, Debug, PartialEq, EnumIter, Serialize, Deserialize)]
enum RecipeSelectPane {
    List,
    Recipe,
    Exchange,
}

/// Persistent key for [ViewState]
#[derive(Debug, Serialize)]
struct ViewStateKey;

impl PersistentKey for ViewStateKey {
    type Value = ViewState;
}

/// Menu actions available in all contexts
#[derive(Copy, Clone, Debug)]
enum PrimaryMenuAction {
    /// Open the collection file in an external editor, jumping to whatever
    /// recipe/folder is currently selected
    EditCollection,
}

/// Screen areas when the sidebar is *not* visible
struct DefaultAreas {
    /// TODO
    headers: [Rect; 2],
    top_pane: Rect,
    bottom_pane: Rect,
}

impl DefaultAreas {
    /// TODO
    fn new(area: Rect) -> Self {
        let [headers, top_pane, bottom_pane] = Layout::vertical([
            Constraint::Length(3),
            Constraint::Fill(1),
            Constraint::Fill(1),
        ])
        .spacing(Spacing::Overlap(1))
        .areas(area);
        let headers = Layout::horizontal([Constraint::Fill(1); 2])
            .spacing(Spacing::Overlap(1))
            .areas(headers);
        Self {
            headers,
            top_pane,
            bottom_pane,
        }
    }
}

/// Screen areas when the sidebar is visible
struct SidebarAreas {
    /// TODO
    headers: [Rect; 1],
    sidebar: Rect,
    top_pane: Rect,
    bottom_pane: Rect,
}

impl SidebarAreas {
    /// TODO
    fn new(area: Rect) -> Self {
        let [side_bar, area] =
            Layout::horizontal([Constraint::Length(30), Constraint::Fill(1)])
                .spacing(Spacing::Overlap(1))
                .areas(area);
        let [headers, top_pane, bottom_pane] = Layout::vertical([
            Constraint::Length(3),
            Constraint::Fill(1),
            Constraint::Fill(1),
        ])
        .spacing(Spacing::Overlap(1))
        .areas(area);
        let headers = Layout::horizontal([Constraint::Fill(1); 1])
            .spacing(Spacing::Overlap(1))
            .areas(headers);
        Self {
            headers,
            sidebar: side_bar,
            top_pane,
            bottom_pane,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        http::RequestConfig,
        message::Message,
        test_util::{TestHarness, TestTerminal, harness, terminal},
        view::test_util::TestComponent,
    };
    use rstest::rstest;
    use slumber_core::http::BuildOptions;
    use slumber_util::assert_matches;
    use terminput::KeyCode;

    /// Create component to be tested
    fn create_component<'term>(
        harness: &mut TestHarness,
        terminal: &'term TestTerminal,
    ) -> TestComponent<'term, PrimaryView> {
        let mut component =
            TestComponent::new(harness, terminal, PrimaryView::new());
        // Initial events
        assert_matches!(
            component.int().drain_draw().propagated(),
            &[Event::HttpSelectRequest(None)]
        );
        // Clear template preview messages so we can test what we want
        harness.clear_messages();
        component
    }

    /// Test selected pane and fullscreen mode loading from persistence
    #[rstest]
    fn test_pane_persistence(
        mut _harness: TestHarness,
        _terminal: TestTerminal,
    ) {
        todo!()
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
        assert_eq!(component.request_config(), Some(expected_config));
    }

    /// Test "Edit Recipe" action
    #[rstest]
    fn test_edit_recipe(mut harness: TestHarness, terminal: TestTerminal) {
        let mut component = create_component(&mut harness, &terminal);
        component.int().drain_draw().assert_empty();

        harness.clear_messages(); // Clear init junk

        component.int().action(&["Edit Recipe"]).assert_empty();
        // Event should be converted into a message appropriately
        assert_matches!(
            harness.pop_message_now(),
            // The actual location is unimportant because the collection was
            // generated in memory, but make sure it's populated
            Message::CollectionEdit { location: Some(_) }
        );
    }

    /// Test actions under the "Copy" submenu
    #[rstest]
    #[case::url("URL", RecipeCopyTarget::Url)]
    #[case::cli("as CLI", RecipeCopyTarget::Cli)]
    #[case::curl("as cURL", RecipeCopyTarget::Curl)]
    #[case::python("as Python", RecipeCopyTarget::Python)]
    fn test_copy_action(
        mut harness: TestHarness,
        terminal: TestTerminal,
        #[case] label: &str,
        #[case] expected_target: RecipeCopyTarget,
    ) {
        let mut component = create_component(&mut harness, &terminal);

        component
            .int()
            .send_key(KeyCode::Char('l')) // Select recipe list
            .action(&["Copy", label])
            .assert_empty();

        let actual_target = assert_matches!(
            harness.pop_message_now(),
            Message::CopyRecipe(target) => target
        );
        assert_eq!(actual_target, expected_target);
    }
}
