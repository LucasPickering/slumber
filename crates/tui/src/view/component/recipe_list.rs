use crate::{
    util::{PersistentKey, PersistentStore},
    view::{
        Generate, ViewContext,
        common::select::SelectFilter,
        component::{
            Component, ComponentId,
            sidebar_list::{Collapse, PrimaryListState},
        },
    },
};
use ratatui::text::Span;
use serde::Serialize;
use slumber_config::Action;
use slumber_core::collection::{
    HasId, RecipeId, RecipeLookupKey, RecipeNode, RecipeNodeType,
};
use std::collections::HashSet;

/// State for a list/tree of recipes and folders. This is mostly just a list,
/// but with some extra logic to allow expanding/collapsing nodes. This is meant
/// to be used as state for [PrimaryList](super::primary_list::PrimaryList).
/// That parent handles some of the expand/collapse logic (e.g. input handling),
/// but some is handled here as well because the implementation is specific
/// to folders and recipes.
#[derive(Debug)]
pub struct RecipeListState {
    id: ComponentId,
    /// Set of all folders that are collapsed
    /// Invariant: No recipes, only folders
    ///
    /// We persist the entire set. This will accrue removed folders over time
    /// (if they were collapsed at the time of deletion). That isn't really an
    /// issue though, it just means it'll be pre-collapsed if the user ever
    /// adds the folder back. Not worth working around.
    collapsed: HashSet<RecipeId>,
}

impl RecipeListState {
    /// Is the given folder collapsed?
    fn is_collapsed(&self, folder_id: &RecipeId) -> bool {
        self.collapsed.contains(folder_id)
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
}

impl Default for RecipeListState {
    fn default() -> Self {
        let collapsed = PersistentStore::get(&CollapsedKey).unwrap_or_default();
        Self {
            id: ComponentId::default(),
            collapsed,
        }
    }
}

impl Component for RecipeListState {
    fn id(&self) -> ComponentId {
        self.id
    }

    // TODO actions

    fn persist(&self, store: &mut PersistentStore) {
        store.set(&CollapsedKey, &self.collapsed);
    }
}

impl PrimaryListState for RecipeListState {
    const TITLE: &str = "Recipe";
    const ACTION: Action = Action::SelectRecipeList;

    type Item = RecipeListItem;
    type PersistentKey = SelectedRecipeKey;

    fn persistent_key(&self) -> Self::PersistentKey {
        SelectedRecipeKey
    }

    fn items(&self) -> Vec<Self::Item> {
        let recipes = &ViewContext::collection().recipes;

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
    }

    /// Set the currently selected folder as expanded/collapsed (or toggle it).
    /// If a folder is not selected, do nothing. Returns whether a change was
    /// made.
    fn collapse(&mut self, selected: &Self::Item, action: Collapse) -> bool {
        if selected.is_folder() {
            let folder = selected;
            let collapsed = &mut self.collapsed;
            match action {
                Collapse::Expand => collapsed.remove(&folder.id),
                Collapse::Collapse => collapsed.insert(folder.id.clone()),
                Collapse::Toggle => {
                    if collapsed.contains(&folder.id) {
                        collapsed.remove(&folder.id);
                    } else {
                        collapsed.insert(folder.id.clone());
                    }
                    true
                }
            }
        } else {
            // Recipe selected - do nothing
            false
        }
    }
}

/// Simplified version of [RecipeNode], to be used in the display tree. This
/// only stores whatever data is necessary to render the list
#[derive(Debug)]
pub struct RecipeListItem {
    id: RecipeId,
    name: String,
    /// The name of this item and *all* of its children, grandchildren, etc.For
    /// This is used during filtering, so that a folder always shows when any
    /// of its children match. This duplicates a lot of strings in the recipe
    /// tree, but the overall size should be very low so it has no meaningful
    /// impact.
    search_terms: Vec<String>,
    kind: RecipeNodeType,
    depth: usize,
    collapsed: bool,
}

impl RecipeListItem {
    fn new(node: &RecipeNode, collapsed: bool, depth: usize) -> Self {
        fn add_search_terms(terms: &mut Vec<String>, node: &RecipeNode) {
            terms.push(node.name().to_owned());
            if let RecipeNode::Folder(folder) = node {
                for child in folder.children.values() {
                    // Recursion!
                    add_search_terms(terms, child);
                }
            }
        }

        let mut search_terms = vec![];
        add_search_terms(&mut search_terms, node);

        Self {
            id: node.id().clone(),
            name: node.name().to_owned(),
            search_terms,
            kind: node.into(),
            collapsed,
            depth,
        }
    }

    pub fn kind(&self) -> RecipeNodeType {
        self.kind
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

impl PartialEq<RecipeId> for RecipeListItem {
    fn eq(&self, id: &RecipeId) -> bool {
        self.id() == id
    }
}

impl SelectFilter for RecipeListItem {
    fn terms(&self) -> Vec<&str> {
        self.search_terms.iter().map(String::as_str).collect()
    }
}

impl Generate for &RecipeListItem {
    type Output<'this>
        = Span<'this>
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

/// Persisted key for the ID of the selected recipe
#[derive(Debug, Serialize)]
pub struct SelectedRecipeKey;

impl PersistentKey for SelectedRecipeKey {
    // Intentionally don't persist None. That's only possible if the recipe map
    // is empty. If it is, we're forced into None. If not, we want to default to
    // the first recipe.
    type Value = RecipeId;
}

/// Persistence key for collapsed state
#[derive(Debug, Default, Serialize)]
struct CollapsedKey;

impl PersistentKey for CollapsedKey {
    type Value = HashSet<RecipeId>;
}
