use crate::{
    collection::{Recipe, RecipeId, RecipeLookupKey, RecipeNode, RecipeTree},
    tui::{
        context::TuiContext,
        input::Action,
        message::MessageSender,
        view::{
            common::Pane,
            component::primary::PrimaryPane,
            draw::{Draw, DrawMetadata, Generate},
            event::{Event, EventHandler, EventQueue, Update},
            state::{
                persistence::{Persistable, Persistent, PersistentKey},
                select::SelectState,
            },
            Component,
        },
    },
};
use derive_more::{Deref, DerefMut};
use itertools::Itertools;
use ratatui::Frame;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

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
    /// A clone of the recipe tree
    recipes: RecipeTree,
    /// The visible list of items is tracked using normal list state, so we can
    /// easily re-use existing logic. We'll rebuild this any time a folder is
    /// expanded/collapsed (i.e whenever the list of items changes)
    select: Component<Persistent<SelectState<RecipeNode>>>,
    /// Set of all folders that are collapsed
    /// Invariant: No recipes, only folders
    collapsed: Persistent<Collapsed>,
}

/// Set of collapsed folders. This newtype is really only necessary so we can
/// implement [Persistable] on it
#[derive(Debug, Default, Deref, DerefMut, Serialize, Deserialize)]
#[serde(transparent)]
struct Collapsed(HashSet<RecipeId>);

/// Ternary state for modifying node collapse state
enum CollapseState {
    Expand,
    Collapse,
    Toggle,
}

impl RecipeListPane {
    pub fn new(recipes: &RecipeTree) -> Self {
        // This clone is unfortunate, but we can't hold onto a reference to the
        // recipes
        let collapsed = Persistent::new(
            PersistentKey::RecipeCollapsed,
            Collapsed::default(),
        );
        let persistent = Persistent::new(
            PersistentKey::RecipeId,
            build_select_state(recipes, &collapsed),
        );
        Self {
            recipes: recipes.clone(),
            select: persistent.into(),
            collapsed,
        }
    }

    /// Which recipe/folder in the list is selected? `None` iff the list is
    /// empty
    pub fn selected_node(&self) -> Option<&RecipeNode> {
        self.select.data().selected()
    }

    /// Which recipe in the recipe list is selected? `None` iff the list is
    /// empty OR a folder is selected.
    pub fn selected_recipe(&self) -> Option<&Recipe> {
        self.selected_node().and_then(RecipeNode::recipe)
    }

    /// Set the currently selected folder as expanded/collapsed (or toggle it).
    /// If a folder is not selected, do nothing. Returns whether a change was
    /// made.
    fn set_selected_collapsed(&mut self, state: CollapseState) -> bool {
        let select = self.select.data_mut();
        let folder = select.selected().and_then(RecipeNode::folder);
        let changed = if let Some(folder) = folder {
            let collapsed = &mut self.collapsed;
            match state {
                CollapseState::Expand => collapsed.remove(&folder.id),
                CollapseState::Collapse => collapsed.insert(folder.id.clone()),
                CollapseState::Toggle => {
                    if collapsed.contains(&folder.id) {
                        collapsed.remove(&folder.id);
                    } else {
                        collapsed.insert(folder.id.clone());
                    }
                    true
                }
            }
        } else {
            false
        };

        // If we changed the set of what is visible, rebuild the list state
        if changed {
            let mut new_select_state =
                build_select_state(&self.recipes, &self.collapsed);
            // Carry over the selection
            if let Some(selected) = select.selected() {
                new_select_state.select(selected.id());
            }
            **select = new_select_state;
        }

        changed
    }
}

impl EventHandler for RecipeListPane {
    fn update(&mut self, _: &MessageSender, event: Event) -> Update {
        let Some(action) = event.action() else {
            return Update::Propagate(event);
        };
        match action {
            Action::LeftClick => {
                EventQueue::push(Event::new_other(PrimaryPane::RecipeList));
            }
            Action::Left => {
                self.set_selected_collapsed(CollapseState::Collapse);
            }
            Action::Right => {
                self.set_selected_collapsed(CollapseState::Expand);
            }
            // If this state update does nothing, then we have a recipe
            // selected. Fall through to propagate the event
            Action::Submit
                if self.set_selected_collapsed(CollapseState::Toggle) => {}
            _ => return Update::Propagate(event),
        }

        Update::Consumed
    }

    fn children(&mut self) -> Vec<Component<&mut dyn EventHandler>> {
        vec![self.select.as_child()]
    }
}

impl Draw for RecipeListPane {
    fn draw(&self, frame: &mut Frame, _: (), metadata: DrawMetadata) {
        let select = self.select.data();
        let context = TuiContext::get();

        let title = context
            .input_engine
            .add_hint("Recipes", Action::SelectRecipeList);
        let pane = Pane {
            title: &title,
            has_focus: metadata.has_focus(),
        };

        // We have to build this manually instead of using our own List type,
        // because we need outside context during the render
        let items = select
            .items()
            .iter()
            .map(|node| {
                let (icon, name) = match node {
                    RecipeNode::Folder(folder) => {
                        let icon = if self.collapsed.is_collapsed(&folder.id) {
                            "▶"
                        } else {
                            "▼"
                        };
                        (icon, folder.name())
                    }
                    RecipeNode::Recipe(recipe) => ("", recipe.name()),
                };
                let depth = self
                    .recipes
                    .get_lookup_key(node.id())
                    .unwrap_or_else(|| {
                        panic!("Recipe node {} is not in tree", node.id())
                    })
                    .as_slice()
                    .len()
                    - 1;

                // Apply indentation
                format!(
                    "{indent:width$}{icon}{name}",
                    indent = "",
                    width = depth
                )
            })
            .collect_vec();
        let list = ratatui::widgets::List::new(items)
            .block(pane.generate())
            .highlight_style(context.styles.list.highlight);
        self.select.draw(frame, list, metadata.area(), true);
    }
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
        let [ancestors @ .., _] = lookup_key.as_slice() else {
            panic!("Recipe lookup key cannot be empty")
        };
        !ancestors.iter().any(|id| self.is_collapsed(id))
    }
}

/// Persist recipe by ID
impl Persistable for RecipeNode {
    type Persisted = RecipeId;

    fn get_persistent(&self) -> &Self::Persisted {
        self.id()
    }
}

/// Needed for persistence loading
impl PartialEq<RecipeNode> for RecipeId {
    fn eq(&self, node: &RecipeNode) -> bool {
        self == node.id()
    }
}

/// Persistence for collapsed set of folders. Technically this can accrue
/// removed folders over time (if they were collapsed at the time of deletion).
/// That isn't really an issue though, it just means it'll be pre-collapsed if
/// the user ever adds the folder back. Not worth working around.
impl Persistable for Collapsed {
    type Persisted = Self;

    fn get_persistent(&self) -> &Self::Persisted {
        self
    }
}

/// Construct select list based on which nodes are currently visible
fn build_select_state(
    recipes: &RecipeTree,
    collapsed: &Collapsed,
) -> SelectState<RecipeNode> {
    // When highlighting a new recipe, load it from the repo
    fn on_select(_: &mut RecipeNode) {
        // If a recipe isn't selected, this will do nothing
        EventQueue::push(Event::HttpLoadRequest);
    }

    let items = recipes
        .iter()
        // Filter out hidden nodes
        .filter(|(lookup_key, _)| collapsed.is_visible(lookup_key))
        .map(|(_, node)| node.clone())
        .collect();
    SelectState::builder(items).on_select(on_select).build()
}
