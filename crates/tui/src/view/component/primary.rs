//! Components for the "primary" view, which is the paned request/response view

use crate::{
    http::RequestState,
    message::Message,
    util::ResultReported,
    view::{
        common::{actions::ActionsModal, modal::ModalHandle},
        component::{
            exchange_pane::{
                ExchangePane, ExchangePaneEvent, ExchangePaneProps,
            },
            help::HelpModal,
            profile_select::ProfilePane,
            recipe_list::{RecipeListPane, RecipeListPaneEvent},
            recipe_pane::{
                RecipeMenuAction, RecipePane, RecipePaneEvent, RecipePaneProps,
            },
        },
        context::UpdateContext,
        draw::{Draw, DrawMetadata, ToStringGenerate},
        event::{Child, Emitter, Event, EventHandler, Update},
        state::{
            fixed_select::FixedSelectState,
            select::{SelectStateEvent, SelectStateEventType},
        },
        util::{
            persistence::{Persisted, PersistedLazy},
            view_text,
        },
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
    Collection, ProfileId, RecipeId, RecipeNodeType,
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
    actions_handle: ModalHandle<ActionsModal<MenuAction>>,
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
        let profile_pane = ProfilePane::new(collection).into();
        let recipe_list_pane = RecipeListPane::new(&collection.recipes).into();

        Self {
            selected_pane: PersistedLazy::new(
                SingletonKey::default(),
                FixedSelectState::builder()
                    .subscribe([SelectStateEventType::Select])
                    .build(),
            ),
            fullscreen_mode: Default::default(),

            recipe_list_pane,
            profile_pane,
            recipe_pane: Default::default(),
            exchange_pane: Default::default(),
            actions_handle: Default::default(),
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

    /// Send a request for the currently selected recipe (if any)
    fn send_request(&self) {
        if let Some(config) = self.recipe_pane.data().request_config() {
            ViewContext::send_message(Message::HttpBeginRequest(config));
        }
    }

    /// Handle menu actions for recipe list or detail panes. We handle this here
    /// for code de-duplication, and because we have access to all the needed
    /// context.
    fn handle_recipe_menu_action(&self, action: RecipeMenuAction) {
        // If no recipes are available, we can't do anything
        let Some(config) = self.recipe_pane.data().request_config() else {
            return;
        };

        match action {
            RecipeMenuAction::EditCollection => {
                ViewContext::send_message(Message::CollectionEdit)
            }
            RecipeMenuAction::CopyUrl => {
                ViewContext::send_message(Message::CopyRequestUrl(config))
            }
            RecipeMenuAction::CopyCurl => {
                ViewContext::send_message(Message::CopyRequestCurl(config))
            }
            RecipeMenuAction::CopyBody => {
                ViewContext::send_message(Message::CopyRequestBody(config))
            }
            RecipeMenuAction::ViewBody => {
                self.recipe_pane.data().with_body_text(view_text)
            }
        }
    }
}

impl EventHandler for PrimaryView {
    fn update(&mut self, _: &mut UpdateContext, event: Event) -> Update {
        if let Some(action) = event.action() {
            match action {
                Action::PreviousPane => self.selected_pane.get_mut().previous(),
                Action::NextPane => self.selected_pane.get_mut().next(),
                // Send a request from anywhere
                Action::Submit => self.send_request(),
                Action::OpenActions => {
                    self.actions_handle.open(ActionsModal::default());
                }
                Action::OpenHelp => {
                    ViewContext::open_modal(HelpModal);
                }

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
                _ => return Update::Propagate(event),
            }
        } else if let Some(event) = self.selected_pane.emitted(&event) {
            if let SelectStateEvent::Select(_) = event {
                // Exit fullscreen when pane changes
                self.maybe_exit_fullscreen();
            }
        } else if let Some(event) = self.recipe_list_pane.emitted(&event) {
            match event {
                RecipeListPaneEvent::Click => {
                    self.selected_pane
                        .get_mut()
                        .select(&PrimaryPane::RecipeList);
                }
                RecipeListPaneEvent::MenuAction(action) => {
                    self.handle_recipe_menu_action(*action);
                }
            }
        } else if let Some(event) = self.recipe_pane.emitted(&event) {
            match event {
                RecipePaneEvent::Click => {
                    self.selected_pane.get_mut().select(&PrimaryPane::Recipe);
                }
                RecipePaneEvent::MenuAction(action) => {
                    self.handle_recipe_menu_action(*action);
                }
            }
        } else if let Some(ExchangePaneEvent::Click) =
            self.exchange_pane.emitted(&event)
        {
            self.selected_pane.get_mut().select(&PrimaryPane::Exchange);
        } else if let Some(action) = self.actions_handle.emitted(&event) {
            // Handle our own menu action type
            match action {
                MenuAction::EditCollection => {
                    ViewContext::send_message(Message::CollectionEdit)
                }
            }
        } else {
            return Update::Propagate(event);
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
            panes.recipe.area,
            panes.recipe.focus,
        );

        self.exchange_pane.draw(
            frame,
            ExchangePaneProps {
                selected_recipe_kind,
                request_state: props.selected_request,
            },
            panes.exchange.area,
            panes.exchange.focus,
        );
    }
}

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
        test_util::{harness, terminal, TestHarness, TestTerminal},
        view::{
            test_util::TestComponent, util::persistence::DatabasePersistedStore,
        },
    };
    use crossterm::event::KeyCode;
    use persisted::PersistedStore;
    use rstest::rstest;
    use slumber_core::{assert_matches, http::BuildOptions};

    /// Create component to be tested
    fn create_component<'term>(
        harness: &mut TestHarness,
        terminal: &'term TestTerminal,
    ) -> TestComponent<'term, PrimaryView, PrimaryViewProps<'static>> {
        let view = PrimaryView::new(&harness.collection);
        let mut component = TestComponent::new(
            harness,
            terminal,
            view,
            PrimaryViewProps {
                selected_request: None,
            },
        );
        // Initial events
        assert_matches!(
            component.drain_draw().events(),
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
            .send_keys([
                KeyCode::Char('l'), // Select recipe list
                KeyCode::Char('x'), // Open actions modal
                KeyCode::Down,
                KeyCode::Enter, // Copy URL
            ])
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
            .send_keys([
                KeyCode::Char('l'), // Select recipe list
                KeyCode::Char('x'), // Open actions modal
                KeyCode::Down,
                KeyCode::Down,
                KeyCode::Down,
                KeyCode::Down,
                KeyCode::Enter, // Copy Body
            ])
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
            .send_keys([
                KeyCode::Char('l'), // Select recipe list
                KeyCode::Char('x'), // Open actions modal
                KeyCode::Down,
                KeyCode::Down,
                KeyCode::Enter, // Copy as cURL
            ])
            .assert_empty();

        let request_config = assert_matches!(
            harness.pop_message_now(),
            Message::CopyRequestCurl(request_config) => request_config,
        );
        assert_eq!(request_config, expected_config);
    }
}
