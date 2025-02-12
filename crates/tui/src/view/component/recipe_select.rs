use crate::{
    context::TuiContext,
    view::{
        common::{
            list::List,
            modal::{Modal, ModalHandle},
            text_box::{TextBox, TextBoxEvent, TextBoxProps},
        },
        context::UpdateContext,
        draw::{Draw, DrawMetadata, Generate},
        event::{Child, Emitter, Event, EventHandler, OptionEvent, ToEmitter},
        state::select::{SelectState, SelectStateEvent, SelectStateEventType},
        util::persistence::Persisted,
        Component, ViewContext,
    },
};
use derive_more::{Deref, DerefMut};
use persisted::{PersistedKey, SingletonKey};
use ratatui::{
    layout::{Constraint, Layout},
    text::{Line, Text},
    Frame,
};
use serde::{Deserialize, Serialize};
use slumber_config::Action;
use slumber_core::collection::{
    HasId, RecipeId, RecipeLookupKey, RecipeNode, RecipeNodeType, RecipeTree,
};
use std::collections::HashSet;

/// Minimal component to show the current recipe, and handle interaction to open
/// the recipe tree modal
#[derive(Debug)]
pub struct RecipeSelect {
    /// Current selected recipe. We have to duplicate this from the modal's
    /// internal select list, because the modal isn't always open.
    selected_recipe_id: Persisted<SelectedRecipeKey>,
    /// Handle events from the opened modal
    modal_handle: ModalHandle<SelectRecipe>,
}

impl RecipeSelect {
    pub fn new(recipes: &RecipeTree) -> Self {
        let mut selected_recipe_id = Persisted::new_default(SelectedRecipeKey);

        // Two invalid cases we need to handle here:
        // - Nothing is persisted but the map has values now
        // - Persisted ID isn't in the map now
        // In either case, just fall back to:
        // - First recipe if available
        // - `None` if map is empty
        match &*selected_recipe_id {
            Some(id) if recipes.contains_id(id) => {}
            _ => {
                *selected_recipe_id.get_mut() =
                    recipes.iter().next().map(|(_, node)| node.id().clone());
            }
        }

        Self {
            selected_recipe_id,
            modal_handle: ModalHandle::new(),
        }
    }

    /// ID and kind of whatever recipe/folder in the list is selected. `None`
    /// iff the list is empty
    pub fn selected_node(&self) -> Option<(&RecipeId, RecipeNodeType)> {
        self.selected_recipe_id.as_ref().and_then(|id| {
            let recipes = &ViewContext::collection().recipes;
            let node = recipes.get(id)?;
            Some((id, node.into()))
        })
    }

    /// Open the recipe list modal
    pub fn open_modal(&mut self) {
        self.modal_handle
            .open(RecipeListModal::new(self.selected_recipe_id.clone()));
    }
}

impl EventHandler for RecipeSelect {
    fn update(&mut self, _: &mut UpdateContext, event: Event) -> Option<Event> {
        event
            .opt()
            .action(|action, propagate| match action {
                Action::LeftClick | Action::SelectRecipeList => {
                    self.open_modal()
                }
                _ => propagate.set(),
            })
            .emitted(
                self.modal_handle.to_emitter(),
                |SelectRecipe(recipe_id)| {
                    *self.selected_recipe_id.get_mut() = Some(recipe_id);
                },
            )
    }
}

impl Draw for RecipeSelect {
    fn draw(&self, frame: &mut Frame, _: (), metadata: DrawMetadata) {
        let label = TuiContext::get()
            .input_engine
            .add_hint("Recipes", Action::SelectRecipeList);

        // Grab global profile selection state
        let collection = ViewContext::collection();
        let selected_node = (*self.selected_recipe_id)
            .as_ref()
            .and_then(|recipe_id| collection.recipes.get(recipe_id));
        frame.render_widget(
            if let Some(node) = selected_node {
                format!("{label}: {}", node.name())
            } else {
                format!("{label}: No recipes defined")
            },
            metadata.area(),
        );
    }
}

/// A modal to allow the user to navigate recipes/folders and select one. This
/// will update the recipe/request panes in the background while the user
/// navigates, so they can see what they're selecting.
#[derive(Debug)]
struct RecipeListModal {
    emitter: Emitter<SelectRecipe>,
    /// The selected recipe when the modal was opened. We'll revert back to
    /// this if the user cancels out of the modal
    initial_selected: Option<RecipeId>,
    tree: Component<Tree>,
    filter: Component<TextBox>,
}

impl RecipeListModal {
    fn new(selected_recipe_id: Option<RecipeId>) -> Self {
        let tree = Tree::new(selected_recipe_id.as_ref());
        let filter = TextBox::default().placeholder("Filter");
        Self {
            emitter: Emitter::default(),
            initial_selected: selected_recipe_id,
            tree: tree.into(),
            filter: filter.into(),
        }
    }

    /// Update global state to select a recipe
    fn select(&mut self, recipe_id: RecipeId) {
        self.emitter.emit(SelectRecipe(recipe_id));
        // Update Request pane as well
        ViewContext::push_event(Event::HttpSelectRequest(None));
    }
}

impl Modal for RecipeListModal {
    fn title(&self) -> Line<'_> {
        "Recipes".into()
    }

    fn dimensions(&self) -> (Constraint, Constraint) {
        // Shrink down to the size of the list, plus 1 for the filter box
        let height = self.tree.data().select.data().len() as u16 + 1;
        (Constraint::Max(40), Constraint::Max(height))
    }

    fn on_close(mut self: Box<Self>, submitted: bool) {
        // If the user cancels out, reset the to the recipe selected when the
        // modal was opened
        // 2024 edition: if-let chain
        match self.initial_selected.clone() {
            Some(initial) if !submitted => self.select(initial),
            _ => {}
        }
    }
}

impl EventHandler for RecipeListModal {
    fn update(&mut self, _: &mut UpdateContext, event: Event) -> Option<Event> {
        event
            .opt()
            .emitted(self.tree.data().emitter, |SelectRecipe(recipe_id)| {
                self.select(recipe_id)
            })
            .emitted(self.filter.to_emitter(), |event| match event {
                TextBoxEvent::Focus => {}
                TextBoxEvent::Change => {
                    self.tree.data_mut().filter =
                        self.filter.data().text().to_lowercase();
                    self.tree.data_mut().rebuild_select_state()
                }
                // Use text box to detect close conditions. Normally this is
                // handled by the modal, but the text box will eat them
                TextBoxEvent::Cancel => self.close(false),
                TextBoxEvent::Submit => self.close(true),
            })
    }

    fn children(&mut self) -> Vec<Component<Child<'_>>> {
        // These two both want to consume left/right actions. If there's text
        // in the filter, we always expand all folders anyway so left/right
        // should navigate the text. If filter is empty, there's no text to
        // navigate so left/right expands/collapses folders instead
        if self.filter.data().text().is_empty() {
            vec![self.tree.to_child_mut(), self.filter.to_child_mut()]
        } else {
            vec![self.filter.to_child_mut(), self.tree.to_child_mut()]
        }
    }
}

impl Draw for RecipeListModal {
    fn draw(&self, frame: &mut Frame, _: (), metadata: DrawMetadata) {
        let [select_area, filter_area] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(1)])
                .areas(metadata.area());

        self.tree.draw(frame, (), select_area, true);

        self.filter
            .draw(frame, TextBoxProps::default(), filter_area, true);
    }
}

impl ToEmitter<SelectRecipe> for RecipeListModal {
    fn to_emitter(&self) -> Emitter<SelectRecipe> {
        self.emitter
    }
}

/// Persisted key for the ID of the selected recipe
#[derive(Debug, Serialize, PersistedKey)]
#[persisted(Option<RecipeId>)]
struct SelectedRecipeKey;

/// Emitted event to pass selected recipe ID from modal back to the parent
#[derive(Debug)]
struct SelectRecipe(RecipeId);

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

/// Helper to handle the select and collapse state. This needs to be a separate
/// component from the modal so it can handle certain input actions with higher
/// priority than the text box does. This could be made into a more generic
/// component, but that adds abstraction that's not necessary because this is
/// the only tree in the app. For similar reasons, we don't use the library
/// tui-tree-widget, because it requires more abstraction that it saves us in
/// code.
///
/// This implementation leans heavily on the fact that all nodes in the tree
/// have a unique ID, which is another reason why it deserves its own
/// implementation.
#[derive(Debug)]
struct Tree {
    emitter: Emitter<SelectRecipe>,
    /// The visible list of items is tracked using normal list state, so we can
    /// easily re-use existing logic. We'll rebuild this any time a folder is
    /// expanded/collapsed (i.e whenever the list of items changes)
    select: Component<SelectState<RecipeListItem>>,
    /// Set of all folders that are collapsed
    /// Invariant: No recipes, only folders
    ///
    /// We persist the entire set. This will accrue removed folders over time
    /// (if they were collapsed at the time of deletion). That isn't really an
    /// issue though, it just means it'll be pre-collapsed if the user ever
    /// adds the folder back. Not worth working around.
    collapsed: Persisted<SingletonKey<Collapsed>>,
    /// Current applied filter. We have to duplicate this from the parent
    /// because we need it to regenerate the select list when
    /// expanding/collapsing folders
    filter: String,
}

impl Tree {
    fn new(selected_recipe_id: Option<&RecipeId>) -> Self {
        let collapsed: Persisted<SingletonKey<Collapsed>> =
            Persisted::default();
        let select = build_select_state(selected_recipe_id, &collapsed, "");
        Self {
            emitter: Emitter::default(),
            select: select.into(),
            collapsed,
            filter: String::new(),
        }
    }

    /// Set the currently selected folder as expanded/collapsed. If a folder is
    /// not selected, do nothing.
    fn set_selected_collapsed(&mut self, collapse: bool) {
        let folder = self
            .select
            .data()
            .selected()
            .filter(|node| node.is_folder());
        let changed = if let Some(folder) = folder {
            let mut collapsed = self.collapsed.get_mut();
            if collapse {
                collapsed.collapse(folder.id.clone())
            } else {
                collapsed.expand(&folder.id)
            }
        } else {
            false
        };

        // If we changed the set of what is visible, rebuild the list state
        if changed {
            self.rebuild_select_state();
        }
    }

    /// Rebuild the select list based on current filter/collapsed state
    fn rebuild_select_state(&mut self) {
        let mut new_select_state = build_select_state(
            self.select.data().selected().map(|item| &item.id),
            &self.collapsed,
            &self.filter,
        );

        // Carry over the selection
        let select = self.select.data_mut();
        if let Some(selected) = select.selected() {
            new_select_state.select(selected.id());
        }
        *select = new_select_state;
    }
}

impl EventHandler for Tree {
    fn update(&mut self, _: &mut UpdateContext, event: Event) -> Option<Event> {
        event
            .opt()
            .action(|action, propagate| match action {
                Action::Left => self.set_selected_collapsed(true),
                Action::Right => self.set_selected_collapsed(false),
                _ => propagate.set(),
            })
            .emitted(self.select.to_emitter(), |event| match event {
                SelectStateEvent::Select(index) => {
                    // Let the parent know whenever a recipe is selected
                    let recipe_id = self.select.data()[index].id.clone();
                    self.emitter.emit(SelectRecipe(recipe_id));
                }
                SelectStateEvent::Submit(_) | SelectStateEvent::Toggle(_) => {}
            })
    }

    fn children(&mut self) -> Vec<Component<Child<'_>>> {
        vec![self.select.to_child_mut()]
    }
}

impl Draw for Tree {
    fn draw(&self, frame: &mut Frame, _: (), metadata: DrawMetadata) {
        self.select.draw(
            frame,
            List::from(self.select.data()),
            metadata.area(),
            true,
        );
    }
}

/// Set of collapsed folders. Newtype allows us to encapsulate some extra
/// functionality
#[derive(Debug, Default, Deref, DerefMut, Serialize, Deserialize)]
#[serde(transparent)]
struct Collapsed(HashSet<RecipeId>);

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

    fn expand(&mut self, recipe_id: &RecipeId) -> bool {
        self.0.remove(recipe_id)
    }

    fn collapse(&mut self, recipe_id: RecipeId) -> bool {
        self.0.insert(recipe_id)
    }
}
/// Construct select list based on which nodes are currently visible
fn build_select_state(
    selected_recipe_id: Option<&RecipeId>,
    collapsed: &Collapsed,
    filter: &str,
) -> SelectState<RecipeListItem> {
    let recipes = &ViewContext::collection().recipes;
    let items = if filter.is_empty() {
        // No filter - calculate visible nodes based on collapsed state
        recipes
            .iter()
            .filter(|(lookup_key, _)| collapsed.is_visible(lookup_key))
            .map(|(lookup_key, node)| {
                RecipeListItem::new(
                    node,
                    collapsed.is_collapsed(node.id()),
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
        .preselect_opt(selected_recipe_id)
        .subscribe([SelectStateEventType::Select])
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_util::{terminal, TestHarness, TestTerminal},
        view::test_util::TestComponent,
    };
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
        let mut component =
            TestComponent::new(&harness, &terminal, RecipeListModal::new(None));
        // Clear initial events
        assert_matches!(
            component.int().drain_draw().events(),
            &[Event::HttpSelectRequest(None)],
        );

        // Find something. Match should be caseless. Should trigger an event to
        // load the latest request
        assert_matches!(
            component.int().send_text("2").events(),
            &[Event::HttpSelectRequest(None)]
        );
        let select = component.data().tree.data().select.data();
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
