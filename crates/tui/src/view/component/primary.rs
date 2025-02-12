//! Components for the "primary" view, which is the paned request/response view

use crate::{
    http::RequestState,
    message::{Message, RequestConfig},
    view::{
        common::{
            actions::{IntoMenuAction, MenuAction},
            modal::Modal,
        },
        component::{
            exchange_pane::{ExchangePane, ExchangePaneEvent},
            help::HelpModal,
            recipe_pane::{RecipePane, RecipePaneEvent, RecipePaneProps},
        },
        context::UpdateContext,
        draw::{Draw, DrawMetadata},
        event::{Child, Emitter, Event, EventHandler, OptionEvent, ToEmitter},
        state::{
            fixed_select::FixedSelectState,
            select::{SelectStateEvent, SelectStateEventType},
        },
        util::persistence::{Persisted, PersistedLazy},
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
use slumber_core::collection::{ProfileId, RecipeNode};
use strum::{EnumCount, EnumIter, IntoEnumIterator};

/// Primary TUI view, which shows request/response panes
#[derive(Debug)]
pub struct PrimaryView {
    // Own state
    selected_pane:
        PersistedLazy<SingletonKey<PrimaryPane>, FixedSelectState<PrimaryPane>>,
    fullscreen_mode: Persisted<FullscreenModeKey>,

    // Children
    recipe_pane: Component<RecipePane>,
    exchange_pane: Component<ExchangePane>,

    global_actions_emitter: Emitter<GlobalMenuAction>,
}

impl PrimaryView {
    pub fn new(selected_request: Option<&RequestState>) -> Self {
        let exchange_pane = ExchangePane::new(selected_request);

        Self {
            selected_pane: PersistedLazy::new(
                SingletonKey::default(),
                FixedSelectState::builder()
                    .subscribe([SelectStateEventType::Select])
                    .build(),
            ),
            fullscreen_mode: Default::default(),

            recipe_pane: Default::default(),
            exchange_pane: exchange_pane.into(),

            global_actions_emitter: Default::default(),
        }
    }

    /// Set the state of the currently selected request. Call whenever a new
    /// request is selected, or the selected request changes state
    pub fn set_request_state(
        &mut self,
        selected_request: Option<&RequestState>,
    ) {
        self.exchange_pane = ExchangePane::new(selected_request).into();
    }

    /// Get a definition of the request that should be sent from the current
    /// recipe settings
    pub fn request_config(&self) -> Option<RequestConfig> {
        self.recipe_pane.data().request_config()
    }

    /// Is the given pane in focus?
    fn is_focused(&self, pane: PrimaryPane) -> bool {
        self.selected_pane.is_selected(&pane)
            || *self.fullscreen_mode == Some(pane)
    }

    fn toggle_fullscreen(&mut self, pane: PrimaryPane) {
        // If we're already in the given mode, exit
        *self.fullscreen_mode.get_mut() = if Some(pane) == *self.fullscreen_mode
        {
            None
        } else {
            Some(pane)
        };
    }

    /// Exit fullscreen mode if it doesn't match the selected pane. This is
    /// called when the pane changes, but it's possible they match when we're
    /// loading from persistence. In those cases, stay in fullscreen.
    fn maybe_exit_fullscreen(&mut self) {
        match (self.selected_pane.selected(), *self.fullscreen_mode) {
            (PrimaryPane::Recipe, Some(PrimaryPane::Recipe))
            | (PrimaryPane::Exchange, Some(PrimaryPane::Exchange)) => {}
            _ => *self.fullscreen_mode.get_mut() = None,
        }
    }

    /// Send a request for the currently selected recipe
    fn send_request(&self) {
        ViewContext::send_message(Message::HttpBeginRequest);
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
                Action::SelectRecipe => {
                    self.selected_pane.get_mut().select(&PrimaryPane::Recipe)
                }
                Action::SelectResponse => {
                    self.selected_pane.get_mut().select(&PrimaryPane::Exchange)
                }

                // Toggle fullscreen
                Action::Fullscreen => match self.selected_pane.selected() {
                    PrimaryPane::Recipe => {
                        self.toggle_fullscreen(PrimaryPane::Recipe)
                    }
                    PrimaryPane::Exchange => {
                        self.toggle_fullscreen(PrimaryPane::Exchange)
                    }
                },
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
            .emitted(self.recipe_pane.to_emitter(), |event| match event {
                RecipePaneEvent::Click => {
                    self.selected_pane.get_mut().select(&PrimaryPane::Recipe);
                }
            })
            .emitted(self.exchange_pane.to_emitter(), |event| match event {
                ExchangePaneEvent::Click => {
                    self.selected_pane.get_mut().select(&PrimaryPane::Exchange)
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
            self.recipe_pane.to_child_mut(),
            self.exchange_pane.to_child_mut(),
        ]
    }
}

impl<'a> Draw<PrimaryViewProps<'a>> for PrimaryView {
    fn draw(
        &self,
        frame: &mut Frame,
        props: PrimaryViewProps,
        metadata: DrawMetadata,
    ) {
        // We draw all panes regardless of fullscreen state, so they can run
        // their necessary state updates. We just give the hidden pane an empty
        // rect to draw into so they don't appear at all
        let area = metadata.area();
        let [recipe_area, exchange_area] = match *self.fullscreen_mode {
            None => Layout::vertical([
                Constraint::Ratio(1, 2),
                Constraint::Ratio(1, 2),
            ])
            .areas(area),
            Some(PrimaryPane::Recipe) => [area, Rect::default()],
            Some(PrimaryPane::Exchange) => [Rect::default(), area],
        };

        self.recipe_pane.draw(
            frame,
            RecipePaneProps {
                selected_recipe_node: props.selected_recipe_node,
                selected_profile_id: props.selected_profile_id,
            },
            recipe_area,
            self.is_focused(PrimaryPane::Recipe),
        );
        self.exchange_pane.draw(
            frame,
            (),
            exchange_area,
            self.is_focused(PrimaryPane::Exchange),
        );
    }
}

#[derive(Clone)]
pub struct PrimaryViewProps<'a> {
    /// ID of the recipe *or* folder selected
    pub selected_recipe_node: Option<&'a RecipeNode>,
    pub selected_profile_id: Option<&'a ProfileId>,
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
    Recipe,
    Exchange,
}

/// Persistence key for fullscreen mode
#[derive(Debug, Default, persisted::PersistedKey, Serialize)]
#[persisted(Option<PrimaryPane>)]
struct FullscreenModeKey;

/// Menu actions available in all contexts
#[derive(Copy, Clone, Debug, Display, EnumIter)]
enum GlobalMenuAction {
    #[display("Edit Collection")]
    EditCollection,
}

impl IntoMenuAction<PrimaryView> for GlobalMenuAction {}

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
    ) -> TestComponent<'term, PrimaryView, ()> {
        let view = PrimaryView::new(&harness.collection, None);
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

    // TODO more tests
    // TODO move tree into submodule
}
