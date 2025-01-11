use crate::{
    context::TuiContext,
    message::Message,
    view::{
        common::{
            actions::{IntoMenuAction, MenuAction},
            list::List,
            text_box::{TextBox, TextBoxEvent, TextBoxProps},
            Pane,
        },
        context::UpdateContext,
        draw::{Draw, DrawMetadata, Generate},
        event::{Child, Emitter, Event, EventHandler, OptionEvent, ToEmitter},
        state::select::{SelectState, SelectStateEvent, SelectStateEventType},
        util::persistence::{Persisted, PersistedLazy},
        Component, ViewContext,
    },
};
use derive_more::{Deref, DerefMut};
use persisted::{PersistedKey, SingletonKey};
use ratatui::{
    layout::{Constraint, Layout},
    text::Text,
    Frame,
};
use serde::{Deserialize, Serialize};
use slumber_config::Action;
use slumber_core::collection::{
    HasId, RecipeId, RecipeLookupKey, RecipeNode, RecipeNodeType, RecipeTree,
};
use std::collections::HashSet;
use strum::{EnumIter, IntoEnumIterator};

/// List/tree of recipes and folders. This is mostly just a list, but with some
/// extra logic to allow expanding/collapsing nodes. This could be made into a
/// more generic component, but that adds abstraction that's not necessary
/// because this is the only tree in the app. For similar reasons, we don't use
/// the library tui-tree-widget, because it requires more abstraction that it
/// saves us in code.
///
/// This implementation leans heavily on the fact that all nodes in the tree
/// have a unique ID, which is another reason why it deserves its own
/// implementation.
#[derive(Debug)]
pub struct RecipeListPane {
    /// Emitter for the on-click event, to focus the pane
    click_emitter: Emitter<RecipeListPaneEvent>,
    /// Emitter for menu actions, to be handled by our parent
    actions_emitter: Emitter<RecipeListMenuAction>,
    /// The visible list of items is tracked using normal list state, so we can
    /// easily re-use existing logic. We'll rebuild this any time a folder is
    /// expanded/collapsed (i.e whenever the list of items changes)
    select: Component<
        PersistedLazy<SelectedRecipeKey, SelectState<RecipeListItem>>,
    >,
    /// Set of all folders that are collapsed
    /// Invariant: No recipes, only folders
    ///
    /// We persist the entire set. This will accrue removed folders over time
    /// (if they were collapsed at the time of deletion). That isn't really an
    /// issue though, it just means it'll be pre-collapsed if the user ever
    /// adds the folder back. Not worth working around.
    collapsed: Persisted<SingletonKey<Collapsed>>,

    filter: Component<TextBox>,
    filter_focused: bool,
}

impl RecipeListPane {
    pub fn new(recipes: &RecipeTree) -> Self {
        let input_engine = &TuiContext::get().input_engine;
        let binding = input_engine.binding_display(Action::Search);

        // This clone is unfortunate, but we can't hold onto a reference to the
        // recipes
        let collapsed: Persisted<SingletonKey<Collapsed>> =
            Persisted::default();
        let select = PersistedLazy::new(
            SelectedRecipeKey,
            collapsed.build_select_state(recipes, ""),
        );
        let filter =
            TextBox::default().placeholder(format!("{binding} to filter"));
        Self {
            click_emitter: Default::default(),
            actions_emitter: Default::default(),
            select: select.into(),
            collapsed,
            filter: filter.into(),
            filter_focused: false,
        }
    }

    /// ID and kind of whatever recipe/folder in the list is selected. `None`
    /// iff the list is empty
    pub fn selected_node(&self) -> Option<(&RecipeId, RecipeNodeType)> {
        self.select
            .data()
            .selected()
            .map(|node| (&node.id, node.kind))
    }

    /// Set the currently selected folder as expanded/collapsed (or toggle it).
    /// If a folder is not selected, do nothing. Returns whether a change was
    /// made.
    fn set_selected_collapsed(&mut self, state: CollapseState) -> bool {
        let folder = self
            .select
            .data()
            .selected()
            .filter(|node| node.is_folder());
        let changed = if let Some(folder) = folder {
            let collapsed = &mut self.collapsed;
            match state {
                CollapseState::Expand => collapsed.get_mut().remove(&folder.id),
                CollapseState::Collapse => {
                    collapsed.get_mut().insert(folder.id.clone())
                }
                CollapseState::Toggle => {
                    if collapsed.contains(&folder.id) {
                        collapsed.get_mut().remove(&folder.id);
                    } else {
                        collapsed.get_mut().insert(folder.id.clone());
                    }
                    true
                }
            }
        } else {
            false
        };

        // If we changed the set of what is visible, rebuild the list state
        if changed {
            self.rebuild_select_state();
        }

        changed
    }

    /// Rebuild the select list based on current filter/collapsed state
    fn rebuild_select_state(&mut self) {
        let mut new_select_state = self.collapsed.build_select_state(
            &ViewContext::collection().recipes,
            &self.filter.data().text().trim().to_lowercase(),
        );

        // Carry over the selection
        let select = self.select.data_mut();
        if let Some(selected) = select.selected() {
            new_select_state.select(selected.id());
        }
        *select.get_mut() = new_select_state;
    }
}

impl EventHandler for RecipeListPane {
    fn update(&mut self, _: &mut UpdateContext, event: Event) -> Option<Event> {
        event
            .opt()
            .action(|action, propagate| match action {
                Action::LeftClick => {
                    self.click_emitter.emit(RecipeListPaneEvent::Click)
                }
                Action::Left => {
                    self.set_selected_collapsed(CollapseState::Collapse);
                }
                Action::Right => {
                    self.set_selected_collapsed(CollapseState::Expand);
                }
                Action::Search => {
                    self.filter_focused = true;
                }
                _ => propagate.set(),
            })
            .emitted(self.select.to_emitter(), |event| match event {
                SelectStateEvent::Select(_) => {
                    // When highlighting a new recipe, load its most recent
                    // request from the DB. If a recipe isn't selected, this
                    // will do nothing
                    ViewContext::push_event(Event::HttpSelectRequest(None));
                }
                SelectStateEvent::Submit(_) => {}
                SelectStateEvent::Toggle(_) => {
                    self.set_selected_collapsed(CollapseState::Toggle);
                }
            })
            .emitted(self.filter.to_emitter(), |event| match event {
                TextBoxEvent::Focus => self.filter_focused = true,
                TextBoxEvent::Change => self.rebuild_select_state(),
                TextBoxEvent::Cancel | TextBoxEvent::Submit => {
                    self.filter_focused = false
                }
            })
            .emitted(self.actions_emitter, |menu_action| match menu_action {
                RecipeListMenuAction::CopyUrl => {
                    ViewContext::send_message(Message::CopyRequestUrl)
                }
                RecipeListMenuAction::CopyCurl => {
                    ViewContext::send_message(Message::CopyRequestCurl)
                }
            })
    }

    fn menu_actions(&self) -> Vec<MenuAction> {
        RecipeListMenuAction::iter()
            .map(MenuAction::with_data(self, self.actions_emitter))
            .collect()
    }

    fn children(&mut self) -> Vec<Component<Child<'_>>> {
        // Filter gets priority if enabled, but users should still be able to
        // navigate the list while filtering
        vec![self.filter.to_child_mut(), self.select.to_child_mut()]
    }
}

impl Draw for RecipeListPane {
    fn draw(&self, frame: &mut Frame, _: (), metadata: DrawMetadata) {
        let context = TuiContext::get();

        let title = context
            .input_engine
            .add_hint("Recipes", Action::SelectRecipeList);
        let block = Pane {
            title: &title,
            has_focus: metadata.has_focus(),
        }
        .generate();
        let area = block.inner(metadata.area());
        frame.render_widget(block, metadata.area());

        let [select_area, filter_area] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(1)])
                .areas(area);

        self.select.draw(
            frame,
            List::from(&**self.select.data()),
            select_area,
            true,
        );

        self.filter.draw(
            frame,
            TextBoxProps::default(),
            filter_area,
            self.filter_focused,
        );
    }
}

/// Notify parent when this pane is clicked
impl ToEmitter<RecipeListPaneEvent> for RecipeListPane {
    fn to_emitter(&self) -> Emitter<RecipeListPaneEvent> {
        self.click_emitter
    }
}

/// Persisted key for the ID of the selected recipe
#[derive(Debug, Serialize, PersistedKey)]
#[persisted(Option<RecipeId>)]
struct SelectedRecipeKey;

/// Emitted event type for the recipe list pane
#[derive(Debug)]
pub enum RecipeListPaneEvent {
    Click,
}

/// Items in the actions popup menu
#[derive(Copy, Clone, Debug, derive_more::Display, EnumIter)]
enum RecipeListMenuAction {
    #[display("Copy URL")]
    CopyUrl,
    #[display("Copy as cURL")]
    CopyCurl,
}

impl IntoMenuAction<RecipeListPane> for RecipeListMenuAction {
    fn enabled(&self, data: &RecipeListPane) -> bool {
        let recipe = data
            .select
            .data()
            .selected()
            .filter(|node| node.is_recipe());
        match self {
            Self::CopyUrl | Self::CopyCurl => recipe.is_some(),
        }
    }
}

/// Simplified version of [RecipeNode], to be used in the display tree. This
/// only stores whatever data is necessary to render the list
#[derive(Debug)]
struct RecipeListItem {
    id: RecipeId,
    name: String,
    kind: RecipeNodeType,
    depth: usize,
    collapsed: bool,
}

impl RecipeListItem {
    fn new(node: &RecipeNode, collapsed: bool, depth: usize) -> Self {
        Self {
            id: node.id().clone(),
            name: node.name().to_owned(),
            kind: node.into(),
            collapsed,
            depth,
        }
    }

    fn is_folder(&self) -> bool {
        matches!(self.kind, RecipeNodeType::Folder)
    }

    fn is_recipe(&self) -> bool {
        matches!(self.kind, RecipeNodeType::Recipe)
    }
}

impl HasId for RecipeListItem {
    type Id = RecipeId;

    fn id(&self) -> &Self::Id {
        &self.id
    }

    fn set_id(&mut self, id: Self::Id) {
        self.id = id;
    }
}

impl PartialEq<RecipeListItem> for RecipeId {
    fn eq(&self, item: &RecipeListItem) -> bool {
        self == item.id()
    }
}

impl<'a> Generate for &'a RecipeListItem {
    type Output<'this> = Text<'this>
    where
        Self: 'this;

    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this,
    {
        let icon = match self.kind {
            RecipeNodeType::Folder if self.collapsed => "▶",
            RecipeNodeType::Folder => "▼",
            RecipeNodeType::Recipe => "",
        };

        // Apply indentation
        format!(
            "{indent:width$}{icon}{name}",
            indent = "",
            name = self.name,
            width = self.depth
        )
        .into()
    }
}

/// Set of collapsed folders. Newtype allows us to encapsulate some extra
/// functionality
#[derive(Debug, Default, Deref, DerefMut, Serialize, Deserialize)]
#[serde(transparent)]
struct Collapsed(HashSet<RecipeId>);

/// Ternary state for modifying node collapse state
enum CollapseState {
    Expand,
    Collapse,
    Toggle,
}

impl Collapsed {
    /// Is this specific folder collapsed?
    fn is_collapsed(&self, folder_id: &RecipeId) -> bool {
        self.0.contains(folder_id)
    }

    /// Is the given node visible? This takes lookup key so it can check all
    /// ancestors for visibility too.
    fn is_visible(&self, lookup_key: &RecipeLookupKey) -> bool {
        // If any ancestors are collapsed, this is *not* visible
        !lookup_key
            .ancestors()
            .iter()
            .any(|id| self.is_collapsed(id))
    }

    /// Construct select list based on which nodes are currently visible
    fn build_select_state(
        &self,
        recipes: &RecipeTree,
        filter: &str,
    ) -> SelectState<RecipeListItem> {
        let items = if filter.is_empty() {
            // No filter - calculate visible nodes based on collapsed state
            recipes
                .iter()
                .filter(|(lookup_key, _)| self.is_visible(lookup_key))
                .map(|(lookup_key, node)| {
                    RecipeListItem::new(
                        node,
                        self.is_collapsed(node.id()),
                        lookup_key.depth(),
                    )
                })
                .collect()
        } else {
            // Find all nodes that match the filter, *and their parents*. If a
            // node is visible we want to show its ancestry too
            let visible: HashSet<RecipeId> = recipes
                .iter()
                .filter(|(_, node)| node.name().to_lowercase().contains(filter))
                // If a node matches, then all its parents should be visible too
                .flat_map(|(lookup_key, _)| lookup_key)
                .collect();

            recipes
                .iter()
                .filter(|(_, node)| visible.contains(node.id()))
                .map(|(lookup_key, node)| {
                    // Never collapse folders here, because we want to show the
                    // user what they're filtering for
                    RecipeListItem::new(node, false, lookup_key.depth())
                })
                .collect()
        };

        SelectState::builder(items)
            .subscribe([
                SelectStateEventType::Select,
                SelectStateEventType::Toggle,
            ])
            .build()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_util::{terminal, TestHarness, TestTerminal},
        view::test_util::TestComponent,
    };
    use crossterm::event::KeyCode;
    use itertools::Itertools;
    use rstest::{fixture, rstest};
    use slumber_core::{
        assert_matches,
        collection::{Collection, Recipe},
        test_util::{by_id, Factory},
    };

    /// Test the filter box
    #[rstest]
    fn test_filter(terminal: TestTerminal, recipes: RecipeTree) {
        // Recipe tree needs to be in ViewContext so it can be used in updates
        let harness = TestHarness::new(Collection {
            recipes,
            ..Collection::factory(())
        });
        let mut component = TestComponent::new(
            &harness,
            &terminal,
            RecipeListPane::new(&harness.collection.recipes),
        );
        // Clear initial events
        assert_matches!(
            component.int().drain_draw().events(),
            &[Event::HttpSelectRequest(None)],
        );

        // Enter filter
        component.int().send_key(KeyCode::Char('/')).assert_empty();
        assert!(component.data().filter_focused);

        // Find something. Match should be caseless. Should trigger an event to
        // load the latest request
        assert_matches!(
            component.int().send_text("2").events(),
            &[Event::HttpSelectRequest(None)]
        );
        let select = component.data().select.data();
        assert_eq!(
            select
                .items()
                .map(|item| &item.id as &str)
                .collect_vec()
                .as_slice(),
            &["recipe2", "recipe22"]
        );
        assert_eq!(
            select.selected().map(|item| &item.id as &str),
            Some("recipe2")
        );

        // Exit filter
        component.int().send_key(KeyCode::Esc).assert_empty();
        assert!(!component.data().filter_focused);
    }

    #[fixture]
    fn recipes() -> RecipeTree {
        by_id([
            Recipe {
                id: "recipe1".into(),
                name: Some("Recipe 1".into()),
                ..Recipe::factory(())
            },
            Recipe {
                id: "recipe2".into(),
                name: Some("Recipe 2".into()),
                ..Recipe::factory(())
            },
            Recipe {
                id: "recipe3".into(),
                name: Some("Recipe 3".into()),
                ..Recipe::factory(())
            },
            Recipe {
                id: "recipe22".into(),
                name: Some("Recipe 22".into()),
                ..Recipe::factory(())
            },
        ])
        .into()
    }
}
