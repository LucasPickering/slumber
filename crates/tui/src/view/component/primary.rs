//! Components for the "primary" view, which is the paned request/response view

mod view_state;

use crate::{
    http::{RequestConfig, RequestState},
    message::{HttpMessage, Message},
    util::ResultReported,
    view::{
        Component, ViewContext,
        common::{actions::MenuItem, modal::ModalQueue},
        component::{
            Canvas, Child, ComponentId, Draw, DrawMetadata, ToChild,
            collection_select::CollectionSelect,
            exchange_pane::ExchangePane,
            primary::view_state::{
                DefaultPane, PrimaryLayout, ProfileSelectPane,
                RecipeSelectPane, ViewState,
            },
            profile::{ProfileDetail, ProfileListState},
            recipe::{RecipeDetail, RecipeList},
            sidebar_list::{SidebarList, SidebarListEvent, SidebarListProps},
        },
        context::UpdateContext,
        event::{Emitter, Event, EventMatch, ToEmitter},
        persistent::{PersistentKey, PersistentStore},
    },
};
use ratatui::{
    layout::{Layout, Spacing},
    prelude::{Constraint, Rect},
};
use serde::Serialize;
use slumber_config::Action;
use slumber_core::collection::{
    ProfileId, RecipeId, RecipeNode, RecipeNodeType,
};

/// Primary TUI view, which shows request/response panes
#[derive(Debug)]
pub struct PrimaryView {
    id: ComponentId,
    // Own state
    /// Current layout and selection state of the view
    view: ViewState,

    // Children
    /// Header/sidebar to select a recipe
    recipe_list: RecipeList,
    /// Recipe preview/detail pane
    recipe_detail: RecipeDetail,
    /// Header/sidebar to select a profile
    profile_list: SidebarList<ProfileListState>,
    /// Profile preview/detail pane
    profile_detail: ProfileDetail,
    /// The exchange pane shows a particular request/response. The entire
    /// component is rebuilt whenever the selected request changes. Internally
    /// it handles non-recipe selections (empty recipe list, folder selected,
    /// etc.) so we don't need to handle that here.
    exchange_pane: ExchangePane,
    /// Modal to select a different collection file
    collection_select: ModalQueue<CollectionSelect>,

    global_actions_emitter: Emitter<PrimaryMenuAction>,
}

impl PrimaryView {
    pub fn new() -> Self {
        let view = PersistentStore::get(&ViewStateKey).unwrap_or_default();

        let recipe_list = RecipeList::default();
        let (recipe_id, recipe_node_type) = recipe_list
            .selected()
            .map(|(id, node_type)| (Some(id), Some(node_type)))
            .unwrap_or((None, None));
        let recipe_detail = Self::build_recipe_detail(recipe_id);

        let profile_list = SidebarList::default();
        let profile_detail = ProfileDetail::new(profile_list.selected_id());

        // We don't have the request store here and there aren't any requests
        // loaded into it yet anyway, so we can't fill out the request yet.
        // There will be a message to load it immediately after though
        let exchange_pane = ExchangePane::new(None, recipe_node_type);

        Self {
            id: ComponentId::default(),
            view,

            recipe_list,
            recipe_detail,
            profile_list,
            profile_detail,
            exchange_pane,
            collection_select: Default::default(),

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
        self.profile_list.selected_id()
    }

    fn selected_recipe_node(&self) -> Option<(&RecipeId, RecipeNodeType)> {
        self.recipe_list.selected()
    }

    /// Get a definition of the request that should be sent from the current
    /// recipe settings
    pub fn request_config(&self) -> Option<RequestConfig> {
        let profile_id = self.selected_profile_id().cloned();
        let recipe_id = self.selected_recipe_id()?.clone();
        let options = self.recipe_detail.build_options()?;
        Some(RequestConfig {
            profile_id,
            recipe_id,
            options,
        })
    }

    /// Send a request for the currently selected recipe
    fn send_request(&self) {
        ViewContext::send_message(HttpMessage::Begin);
    }

    /// Refresh the recipe preview. Call this whenever the selected recipe *or*
    /// profile changes
    fn refresh_recipe(&mut self) {
        let collection = ViewContext::collection();
        let selected_recipe_node =
            self.selected_recipe_node().and_then(|(id, _)| {
                collection
                    .recipes
                    .try_get(id)
                    .reported(&ViewContext::messages_tx())
            });
        self.recipe_detail = RecipeDetail::new(selected_recipe_node);

        // When the recipe/profile changes, we want to select the most recent
        // recipe for that combo as well
        ViewContext::push_event(Event::HttpSelectRequest(None));
    }

    /// Update the Exchange pane with the selected request. Call this whenever
    /// a new request is selected.
    pub fn refresh_request(&mut self, selected_request: Option<&RequestState>) {
        self.exchange_pane = ExchangePane::new(
            selected_request,
            self.selected_recipe_node().map(|(_, node_type)| node_type),
        );

        // There are new prompts, jump to the prompt form
        let has_prompts = matches!(
            selected_request,
            Some(RequestState::Building { prompts, .. }) if !prompts.is_empty(),
        );
        if has_prompts {
            self.view.select_exchange_pane();
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

    fn build_recipe_detail(recipe_id: Option<&RecipeId>) -> RecipeDetail {
        let collection = ViewContext::collection();
        let node = recipe_id.and_then(|id| {
            collection
                .recipes
                .try_get(id)
                .reported(&ViewContext::messages_tx())
        });
        RecipeDetail::new(node)
    }
}

impl Component for PrimaryView {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn update(
        &mut self,
        context: &mut UpdateContext,
        event: Event,
    ) -> EventMatch {
        event
            .m()
            .click(|position, _| {
                if self.recipe_detail.contains(context, position) {
                    self.view.select_recipe_pane();
                } else if self.profile_detail.contains(context, position) {
                    self.view.select_profile_pane();
                } else if self.exchange_pane.contains(context, position) {
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
                Action::Cancel if self.view.is_fullscreen() => {
                    self.view.exit_fullscreen();
                }
                _ => propagate.set(),
            })
            .emitted(self.recipe_list.to_emitter(), |event| match event {
                SidebarListEvent::Open => self.view.open_recipe_list(),
                SidebarListEvent::Select => self.refresh_recipe(),
                SidebarListEvent::Close => self.view.close_sidebar(),
            })
            .emitted(self.profile_list.to_emitter(), |event| match event {
                SidebarListEvent::Open => self.view.open_profile_list(),
                SidebarListEvent::Select => {
                    // Both panes can change when the profile changes
                    self.profile_detail =
                        ProfileDetail::new(self.profile_list.selected_id());
                    self.refresh_recipe();
                }
                SidebarListEvent::Close => self.view.close_sidebar(),
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
            self.collection_select.to_child_mut(),
            // Not modals
            self.recipe_list.to_child_mut(),
            self.recipe_detail.to_child_mut(),
            self.profile_list.to_child_mut(),
            self.profile_detail.to_child_mut(),
            self.exchange_pane.to_child_mut(),
        ]
    }
}

impl Draw for PrimaryView {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        let area = metadata.area();
        let fullscreen = self.view.is_fullscreen();
        match self.view.layout() {
            // Sidebar is closed
            PrimaryLayout::Default(pane) if fullscreen => match pane {
                DefaultPane::Recipe => {
                    canvas.draw(&self.recipe_detail, (), area, true);
                }
                DefaultPane::Exchange => {
                    canvas.draw(&self.exchange_pane, (), area, true);
                }
            },
            PrimaryLayout::Default(selected_pane) => {
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
                    &self.recipe_detail,
                    (),
                    areas.top_pane,
                    selected_pane == DefaultPane::Recipe,
                );
                canvas.draw(
                    &self.exchange_pane,
                    (),
                    areas.bottom_pane,
                    selected_pane == DefaultPane::Exchange,
                );
            }

            // Profile list is open in sidebar
            PrimaryLayout::Profile(pane) if fullscreen => match pane {
                ProfileSelectPane::List => canvas.draw(
                    &self.profile_list,
                    SidebarListProps::list(),
                    area,
                    true,
                ),
                ProfileSelectPane::Recipe => {
                    canvas.draw(&self.recipe_detail, (), area, true);
                }
                ProfileSelectPane::Profile => {
                    canvas.draw(&self.profile_detail, (), area, true);
                }
            },
            PrimaryLayout::Profile(selected_pane) => {
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
                    selected_pane == ProfileSelectPane::List,
                );

                // Panes
                canvas.draw(
                    &self.recipe_detail,
                    (),
                    areas.top_pane,
                    selected_pane == ProfileSelectPane::Recipe,
                );
                canvas.draw(
                    &self.profile_detail,
                    (),
                    areas.bottom_pane,
                    selected_pane == ProfileSelectPane::Profile,
                );
            }

            // Recipe list is open in sidebar
            PrimaryLayout::Recipe(pane) if fullscreen => match pane {
                RecipeSelectPane::List => canvas.draw(
                    &self.recipe_list,
                    SidebarListProps::list(),
                    area,
                    true,
                ),
                RecipeSelectPane::Recipe => {
                    canvas.draw(&self.recipe_detail, (), area, true);
                }
                RecipeSelectPane::Exchange => {
                    canvas.draw(&self.exchange_pane, (), area, true);
                }
            },
            PrimaryLayout::Recipe(selected_pane) => {
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
                    selected_pane == RecipeSelectPane::List,
                );

                // Panes
                canvas.draw(
                    &self.recipe_detail,
                    (),
                    areas.top_pane,
                    selected_pane == RecipeSelectPane::Recipe,
                );
                canvas.draw(
                    &self.exchange_pane,
                    (),
                    areas.bottom_pane,
                    selected_pane == RecipeSelectPane::Exchange,
                );
            }
        }

        // Modals!!
        canvas.draw_portal(&self.collection_select, (), true);
    }
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
///
/// +---------+
/// | HEADERS |
/// +---------+
/// |         |
/// |   TOP   |
/// +---------+
/// |         |
/// | BOTTOM  |
/// +---------+
struct DefaultAreas {
    /// Evenly divided top row to contain all collapsed lists
    headers: [Rect; 2],
    top_pane: Rect,
    bottom_pane: Rect,
}

impl DefaultAreas {
    /// Split the area into the default layout
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
///
/// +---+---------+
/// | S | HEADERS |
/// | I +---------+
/// | D |         |
/// | E |   TOP   |
/// | B +---------+
/// | A |         |
/// | R | BOTTOM  |
/// +---+---------+
struct SidebarAreas {
    /// Evenly divided top row to contain all the collapsed lists, which
    /// excludes the one list that is expanded in the sidebar.
    headers: [Rect; 1],
    sidebar: Rect,
    top_pane: Rect,
    bottom_pane: Rect,
}

impl SidebarAreas {
    /// Split the area into the sidebar layout
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
        message::{Message, RecipeCopyTarget},
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
            // The profile and recipe lists each trigger this once
            &[
                Event::HttpSelectRequest(None),
                Event::HttpSelectRequest(None)
            ]
        );
        // Clear template preview messages so we can test what we want
        harness.clear_messages();
        component
    }

    /// Test selected pane and fullscreen mode loading from persistence
    #[rstest]
    fn test_pane_persistence(mut harness: TestHarness, terminal: TestTerminal) {
        let mut view = ViewState::default();
        view.select_exchange_pane();
        view.toggle_fullscreen();
        harness.persistent_store().set(&ViewStateKey, &view);

        let component = create_component(&mut harness, &terminal);
        assert_eq!(component.view, view);
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

    /// Test actions under the "Copy" submenu. This should be available in
    /// both the recipe list and recipe detail pane
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
            .send_key(KeyCode::Char('c')) // Select recipe detail
            .action(&["Copy", label])
            .assert_empty();

        let actual_target = assert_matches!(
            harness.pop_message_now(),
            Message::CopyRecipe(target) => target
        );
        assert_eq!(actual_target, expected_target);

        component
            .int()
            .send_key(KeyCode::Char('2')) // Select recipe list
            .action(&["Copy", label])
            .assert_empty();

        let actual_target = assert_matches!(
            harness.pop_message_now(),
            Message::CopyRecipe(target) => target
        );
        assert_eq!(actual_target, expected_target);
    }
}
