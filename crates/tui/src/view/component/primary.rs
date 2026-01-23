//! Components for the "primary" view, which is the paned request/response view

mod view_state;

use crate::{
    http::{RequestConfig, RequestState, RequestStore},
    message::{HttpMessage, Message},
    util::ResultReported,
    view::{
        Component, RequestDisposition, ViewContext,
        common::actions::MenuItem,
        component::{
            Canvas, Child, ComponentExt, ComponentId, Draw, DrawMetadata,
            ToChild,
            exchange_pane::ExchangePane,
            history::History,
            primary::view_state::{
                DefaultPane, PrimaryLayout, Sidebar, SidebarPane, ViewState,
            },
            profile::{ProfileDetail, ProfileListState},
            recipe::{RecipeDetail, RecipeList},
            sidebar_list::{SidebarList, SidebarListEvent, SidebarListProps},
        },
        context::UpdateContext,
        event::{BroadcastEvent, Emitter, Event, EventMatch, ToEmitter},
        persistent::{PersistentKey, PersistentStore},
    },
};
use indexmap::IndexMap;
use ratatui::{
    layout::{Layout, Rect, Spacing},
    prelude::Constraint,
};
use serde::Serialize;
use slumber_config::Action;
use slumber_core::{
    collection::{ProfileId, RecipeId, RecipeNode, RecipeNodeType},
    http::RequestId,
};
use slumber_template::Template;
use slumber_util::yaml::SourceLocation;
use std::iter;

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
    /// List of all past requests for the current recipe/profile
    history: History,

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
        let profile_id = profile_list.selected_id();
        let profile_detail = ProfileDetail::new(profile_id);

        // We don't have the request store here and there aren't any requests
        // loaded into it yet anyway, so we can't fill out the request yet.
        // There will be a message to load it immediately after though
        let exchange_pane = ExchangePane::new(None, recipe_node_type);

        let history = History::new(
            profile_list.selected_id().cloned(),
            recipe_list.selected_recipe_id().cloned(),
        );

        Self {
            id: ComponentId::default(),
            view,

            recipe_list,
            recipe_detail,
            profile_list,
            profile_detail,
            exchange_pane,
            history,

            global_actions_emitter: Default::default(),
        }
    }

    /// Which recipe in the recipe list is selected? `None` iff the list is
    /// empty OR a folder is selected.
    pub fn selected_recipe_id(&self) -> Option<&RecipeId> {
        self.recipe_list.selected_recipe_id()
    }

    /// ID of the selected profile. `None` iff the list is empty
    pub fn selected_profile_id(&self) -> Option<&ProfileId> {
        self.profile_list.selected_id()
    }

    /// ID of the selected request. `None` iff the list of requests is empty
    pub fn selected_request_id(&self) -> Option<RequestId> {
        self.history.selected_id()
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

    /// Get a map of overridden profile fields
    pub fn profile_overrides(&self) -> IndexMap<String, Template> {
        self.profile_detail.overrides()
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
    }

    /// Update the UI to reflect the current state of an HTTP request
    pub fn refresh_request(
        &mut self,
        store: &mut RequestStore,
        disposition: RequestDisposition,
    ) {
        // Refresh history list. This has to happen first so the
        // select_request() call below has access to the latest request
        self.history.refresh(store);

        match disposition {
            RequestDisposition::Change(request_id) => {
                // If the selected request was changed, rebuild state.
                // Otherwise, we don't care about the change
                if Some(request_id) == self.selected_request_id() {
                    // If the request isn't in the store, that means it was just
                    // deleted
                    let state = store.get(request_id);
                    self.set_request(state);
                }
            }
            RequestDisposition::ChangeAll(request_ids) => {
                // Check if the selected request changed
                if let Some(request_id) = self.selected_request_id()
                    && request_ids.contains(&request_id)
                {
                    // If the request isn't in the store, that means it was just
                    // deleted
                    let state = store.get(request_id);
                    self.set_request(state);
                }
            }
            RequestDisposition::Select(request_id) => {
                let Some(state) = store.get(request_id) else {
                    // If the request is not in the store, it can't be selected
                    return;
                };

                // Select only if it matches the current recipe/profile
                let selected_recipe_id = self.selected_recipe_id();
                if state.profile_id() == self.selected_profile_id()
                    && Some(state.recipe_id()) == selected_recipe_id
                {
                    self.history.select_request(state.id());
                }
            }
            RequestDisposition::OpenForm(request_id) => {
                // If a new prompt appears for a request that isn't selected, we
                // *don't* want to switch to it
                if Some(request_id) == self.selected_request_id() {
                    // State *should* be Some here because the form just updated
                    let state = store.get(request_id);
                    // Update the view with the new prompt
                    self.set_request(state);
                    // Select the form pane
                    self.view.select_exchange_pane();
                }
            }
        }
    }

    /// Update the Exchange pane with the selected request. Call this whenever
    /// a new request is selected or the selected request changes.
    fn set_request(&mut self, selected_request: Option<&RequestState>) {
        self.exchange_pane = ExchangePane::new(
            selected_request,
            self.selected_recipe_node().map(|(_, node_type)| node_type),
        );
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

    /// Should a cancel action close the sidebar?
    fn can_close_sidebar(&self, request_store: &RequestStore) -> bool {
        // If the sidebar is open and the request is *not* cancellable. We want
        // request cancelling to take priority over closing the sidebar, but
        // our parent has to handle the cancel action because that's where the
        // necessary context is.
        self.view.sidebar().is_some()
            && self
                .exchange_pane
                .request_id()
                .is_some_and(|request_id| !request_store.can_cancel(request_id))
    }

    /// Draw the selected pane in fullscreen mode
    fn draw_fullscreen(&self, canvas: &mut Canvas, area: Rect) {
        let sidebar_props = SidebarListProps::list();
        match self.view.layout() {
            // Sidebar
            PrimaryLayout::Sidebar {
                sidebar,
                selected_pane: SidebarPane::Sidebar,
            } => match sidebar {
                Sidebar::Profile => {
                    canvas.draw(&self.profile_list, sidebar_props, area, true);
                }
                Sidebar::Recipe => {
                    canvas.draw(&self.recipe_list, sidebar_props, area, true);
                }
                Sidebar::History => canvas.draw(&self.history, (), area, true),
            },
            // Top Pane - always Recipe
            PrimaryLayout::Default(DefaultPane::Top)
            | PrimaryLayout::Sidebar {
                selected_pane: SidebarPane::Top,
                ..
            } => canvas.draw(&self.recipe_detail, (), area, true),
            // Bottom Pane - Exchange or Profile, depending on sidebar
            PrimaryLayout::Default(DefaultPane::Bottom)
            | PrimaryLayout::Sidebar {
                sidebar: Sidebar::Recipe | Sidebar::History,
                selected_pane: SidebarPane::Bottom,
            } => canvas.draw(&self.exchange_pane, (), area, true),
            PrimaryLayout::Sidebar {
                sidebar: Sidebar::Profile,
                selected_pane: SidebarPane::Bottom,
            } => canvas.draw(&self.profile_detail, (), area, true),
        }
    }

    /// Draw the default layout
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
    fn draw_default(
        &self,
        canvas: &mut Canvas,
        area: Rect,
        selected_pane: DefaultPane,
    ) {
        let headers: &[&dyn Draw<_>] = &[&self.profile_list, &self.recipe_list];

        let [headers_area, top_area, bottom_area] = Layout::vertical([
            Constraint::Length(3),
            Constraint::Fill(1),
            Constraint::Fill(1),
        ])
        .spacing(Spacing::Overlap(1))
        .areas(area);
        let headers_areas = Layout::horizontal(iter::repeat_n(
            Constraint::Fill(1),
            headers.len(),
        ))
        .spacing(Spacing::Overlap(1))
        .split(headers_area);

        // Header
        for (header, area) in headers.iter().zip(&*headers_areas) {
            let header_props = SidebarListProps::header();
            canvas.draw(*header, header_props, *area, false);
        }

        // Panes
        canvas.draw(
            &self.recipe_detail,
            (),
            top_area,
            selected_pane == DefaultPane::Top,
        );
        canvas.draw(
            &self.exchange_pane,
            (),
            bottom_area,
            selected_pane == DefaultPane::Bottom,
        );
    }

    /// Draw the sidebar layout
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
    fn draw_sidebar(
        &self,
        canvas: &mut Canvas,
        area: Rect,
        sidebar: Sidebar,
        selected_pane: SidebarPane,
    ) {
        let headers: &[&dyn Draw<_>] = match sidebar {
            Sidebar::Profile => &[&self.recipe_list],
            Sidebar::Recipe => &[&self.profile_list],
            Sidebar::History => &[&self.profile_list, &self.recipe_list],
        };

        // Split the areas
        let [sidebar_area, rest] =
            Layout::horizontal([Constraint::Length(30), Constraint::Fill(1)])
                .spacing(Spacing::Overlap(1))
                .areas(area);
        let [headers_area, top_area, bottom_area] = Layout::vertical([
            Constraint::Length(3),
            Constraint::Fill(1),
            Constraint::Fill(1),
        ])
        .spacing(Spacing::Overlap(1))
        .areas(rest);
        let headers_areas = Layout::horizontal(iter::repeat_n(
            Constraint::Fill(1),
            headers.len(),
        ))
        .spacing(Spacing::Overlap(1))
        .split(headers_area);

        // Header
        for (header, area) in headers.iter().zip(&*headers_areas) {
            let header_props = SidebarListProps::header();
            canvas.draw(*header, header_props, *area, false);
        }

        // Sidebar
        let sidebar_selected = selected_pane == SidebarPane::Sidebar;
        match sidebar {
            Sidebar::Profile => canvas.draw(
                &self.profile_list,
                SidebarListProps::list(),
                sidebar_area,
                sidebar_selected,
            ),
            Sidebar::Recipe => canvas.draw(
                &self.recipe_list,
                SidebarListProps::list(),
                sidebar_area,
                sidebar_selected,
            ),
            Sidebar::History => {
                canvas.draw(&self.history, (), sidebar_area, sidebar_selected);
            }
        }

        // Panes
        canvas.draw(
            &self.recipe_detail,
            (),
            top_area,
            selected_pane == SidebarPane::Top,
        );
        let bottom: &dyn Draw = match sidebar {
            Sidebar::Profile => &self.profile_detail,
            Sidebar::Recipe | Sidebar::History => &self.exchange_pane,
        };
        canvas.draw(
            bottom,
            (),
            bottom_area,
            selected_pane == SidebarPane::Bottom,
        );
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
                Action::History => self.view.open_sidebar(Sidebar::History),
                Action::SelectProfileList => {
                    self.view.open_sidebar(Sidebar::Profile);
                }
                Action::SelectRecipeList => {
                    self.view.open_sidebar(Sidebar::Recipe);
                }
                Action::SelectTopPane => self.view.select_top_pane(),
                Action::SelectBottomPane => self.view.select_bottom_pane(),

                // Toggle fullscreen
                Action::Fullscreen => self.view.toggle_fullscreen(),
                // Exit fullscreen
                Action::Cancel if self.view.is_fullscreen() => {
                    self.view.exit_fullscreen();
                }
                // Close sidebar if it's open, regardless of the selected pane
                Action::Cancel
                    if self.can_close_sidebar(context.request_store) =>
                {
                    self.view.close_sidebar();
                }
                _ => propagate.set(),
            })
            .broadcast(|event| match event {
                // Refresh previews when selected profile/recipe changes
                BroadcastEvent::SelectedProfile(_) => {
                    // Both panes can change when the profile changes
                    self.profile_detail =
                        ProfileDetail::new(self.profile_list.selected_id());
                    self.refresh_recipe();
                }
                BroadcastEvent::SelectedRecipe(_) => self.refresh_recipe(),
                BroadcastEvent::SelectedRequest(request_id) => {
                    // When a new request is selected, make sure it's loaded
                    // from the DB, then put it in the Exchange pane
                    let state = request_id.and_then(|id| {
                        context
                            .request_store
                            .load(id)
                            .reported(&ViewContext::messages_tx())
                            .flatten()
                    });
                    self.set_request(state);
                }
                BroadcastEvent::RefreshPreviews => {}
            })
            .emitted(self.recipe_list.to_emitter(), |event| match event {
                SidebarListEvent::Open => {
                    self.view.open_sidebar(Sidebar::Recipe);
                }
                SidebarListEvent::Select => {
                    ViewContext::push_event(BroadcastEvent::SelectedRecipe(
                        self.selected_recipe_id().cloned(),
                    ));
                }
                SidebarListEvent::Close => self.view.close_sidebar(),
            })
            .emitted(self.profile_list.to_emitter(), |event| match event {
                SidebarListEvent::Open => {
                    self.view.open_sidebar(Sidebar::Profile);
                }
                SidebarListEvent::Select => {
                    ViewContext::push_event(BroadcastEvent::SelectedProfile(
                        self.selected_profile_id().cloned(),
                    ));
                }
                SidebarListEvent::Close => self.view.close_sidebar(),
            })
            // Handle our own menu action type
            .emitted(self.global_actions_emitter, |menu_action| {
                match menu_action {
                    PrimaryMenuAction::EditCollection(location) => {
                        // Forward to the main loop so it can open the editor
                        ViewContext::send_message(Message::CollectionEdit {
                            location,
                        });
                    }
                }
            })
    }

    fn menu(&self) -> Vec<MenuItem> {
        let emitter = self.global_actions_emitter;
        let collection = ViewContext::collection();
        let selected_recipe_node = self
            .selected_recipe_node()
            .and_then(|(id, _)| collection.recipes.get(id));
        let edit_recipe = match selected_recipe_node {
            None => emitter.menu(
                PrimaryMenuAction::EditCollection(None),
                "Edit Collection",
            ),
            Some(RecipeNode::Folder(folder)) => emitter.menu(
                PrimaryMenuAction::EditCollection(Some(
                    folder.location.clone(),
                )),
                "Edit Folder",
            ),
            Some(RecipeNode::Recipe(recipe)) => emitter.menu(
                PrimaryMenuAction::EditCollection(Some(
                    recipe.location.clone(),
                )),
                "Edit Recipe",
            ),
        };
        let profile_location = self.selected_profile_id().and_then(|id| {
            let profile = collection.profiles.get(id)?;
            Some(&profile.location)
        });
        let edit_profile = emitter
            .menu(
                PrimaryMenuAction::EditCollection(profile_location.cloned()),
                "Edit Profile",
            )
            .enable(profile_location.is_some());

        vec![edit_recipe.into(), edit_profile.into()]
    }

    fn persist(&self, store: &mut PersistentStore) {
        store.set(&ViewStateKey, &self.view);
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        vec![
            self.recipe_list.to_child_mut(),
            self.recipe_detail.to_child_mut(),
            self.profile_list.to_child_mut(),
            self.profile_detail.to_child_mut(),
            self.exchange_pane.to_child_mut(),
            self.history.to_child_mut(),
        ]
    }
}

impl Draw for PrimaryView {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        let area = metadata.area();

        if self.view.is_fullscreen() {
            // Fullscreen - just a single pane
            self.draw_fullscreen(canvas, area);
        } else {
            // Multi-pane layouts
            match self.view.layout() {
                PrimaryLayout::Default(selected_pane) => {
                    self.draw_default(canvas, area, selected_pane);
                }
                PrimaryLayout::Sidebar {
                    sidebar,
                    selected_pane,
                } => {
                    self.draw_sidebar(canvas, area, sidebar, selected_pane);
                }
            }
        }
    }
}

/// Persistent key for [ViewState]
#[derive(Debug, Serialize)]
struct ViewStateKey;

impl PersistentKey for ViewStateKey {
    type Value = ViewState;
}

/// Menu actions available in all contexts
#[derive(Clone, Debug)]
enum PrimaryMenuAction {
    /// Open the collection file in an external editor, jumping to the
    /// specified location (if any)
    EditCollection(Option<SourceLocation>),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        http::RequestConfig,
        message::{Message, RecipeCopyTarget},
        test_util::{TestTerminal, terminal},
        view::test_util::{TestComponent, TestHarness, harness},
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
        let recipe_id = harness.collection.first_recipe_id().clone();
        let profile_id = harness.collection.first_profile_id().clone();
        let component =
            TestComponent::builder(harness, terminal, PrimaryView::new())
                .with_default_props()
                // Initial events
                .with_assert_events(|assert| {
                    assert.broadcast([
                        BroadcastEvent::SelectedRecipe(Some(recipe_id)),
                        BroadcastEvent::SelectedProfile(Some(profile_id)),
                        // Two events above each trigger a request selection
                        BroadcastEvent::SelectedRequest(None),
                        BroadcastEvent::SelectedRequest(None),
                    ]);
                })
                .build();
        // Clear template preview messages so we can test what we want
        harness.messages().clear();
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
        component.int().drain_draw().assert().empty();
        harness.messages().clear(); // Clear init junk
        let expected_location =
            harness.collection.first_recipe().location.clone();

        component.int().action(&["Edit Recipe"]).assert().empty();
        // Event should be converted into a message appropriately
        let location = assert_matches!(
            harness.messages().pop_now(),
            Message::CollectionEdit { location: Some(location) } => location
        );
        assert_eq!(location, expected_location);
    }

    /// Test "Edit Profile" action
    #[rstest]
    fn test_edit_profile(mut harness: TestHarness, terminal: TestTerminal) {
        let mut component = create_component(&mut harness, &terminal);
        component.int().drain_draw().assert().empty();
        harness.messages().clear(); // Clear init junk
        let expected_location =
            harness.collection.first_profile().location.clone();

        component.int().action(&["Edit Profile"]).assert().empty();
        // Event should be converted into a message appropriately
        let location = assert_matches!(
            harness.messages().pop_now(),
            Message::CollectionEdit { location: Some(location) } => location
        );
        assert_eq!(location, expected_location);
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
            .send_key(KeyCode::Char('1')) // Select recipe detail
            .action(&["Copy", label])
            .assert()
            .empty();

        let actual_target = assert_matches!(
            harness.messages().pop_now(),
            Message::CopyRecipe(target) => target
        );
        assert_eq!(actual_target, expected_target);

        component
            .int()
            .send_key(KeyCode::Char('r')) // Select recipe list
            .action(&["Copy", label])
            .assert()
            .empty();

        let actual_target = assert_matches!(
            harness.messages().pop_now(),
            Message::CopyRecipe(target) => target
        );
        assert_eq!(actual_target, expected_target);
    }
}
