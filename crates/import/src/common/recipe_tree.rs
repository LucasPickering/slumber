//! Recipe/folder tree structure

use crate::common::{
    Folder, Recipe, RecipeId,
    cereal::{HasId, deserialize_id_map},
};
use indexmap::{IndexMap, map::Values};
use serde::{Deserialize, Deserializer, de::Error};
use strum::EnumDiscriminants;
use thiserror::Error;

// TODO can we simplify this?

/// A folder/recipe tree. This is exactly what the user inputs in their
/// collection file. IDs in this tree are **globally* unique, meaning no two
/// nodes can have the same ID anywhere in the tree, even between folders and
/// recipes. This is a mild restriction on the user that makes implementing a
/// lot simpler. In reality it's unlikely they would want to give two things
/// the same ID anyway.
#[derive(Debug, Default)]
pub(crate) struct RecipeTree {
    /// Tree structure storing all the folder/recipe data
    pub tree: IndexMap<RecipeId, RecipeNode>,
    /// A flattened version of the tree, with each ID pointing to its path in
    /// the tree. This is possible because the IDs are globally unique. It is
    /// an invariant that every lookup key in this map is valid, therefore it's
    /// safe to panic if one is found to be invalid.
    nodes_by_id: IndexMap<RecipeId, RecipeLookupKey>,
}

/// A path into the recipe tree. Every constructed path is assumed to be valid,
/// which must be enforced by the creator.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct RecipeLookupKey(Vec<RecipeId>);

impl From<&Vec<&RecipeId>> for RecipeLookupKey {
    fn from(value: &Vec<&RecipeId>) -> Self {
        Self(value.iter().copied().cloned().collect())
    }
}

impl IntoIterator for RecipeLookupKey {
    type Item = RecipeId;
    type IntoIter = <Vec<RecipeId> as IntoIterator>::IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

/// A node in the recipe tree, either a folder or recipe
#[derive(Debug, Deserialize, EnumDiscriminants)]
#[strum_discriminants(name(RecipeNodeType))]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[allow(clippy::large_enum_variant)]
pub enum RecipeNode {
    Folder(Folder),
    /// Rename this variant to match the `requests` field in the root and
    /// folders
    #[serde(rename = "request")]
    Recipe(Recipe),
}

/// Error returned when attempting to build a [RecipeTree] with a duplicate
/// recipe ID. IDs are unique throughout the entire tree.
#[derive(Debug, Error)]
#[error(
    "Duplicate recipe/folder ID `{0}`; \
    recipe/folder IDs must be globally unique"
)]
pub struct DuplicateRecipeIdError(RecipeId);

impl RecipeTree {
    /// Create a new tree. If there are *any* duplicate IDs in the tree, the
    /// duplicate ID will be returned as an `Err`.
    pub(crate) fn new(
        tree: IndexMap<RecipeId, RecipeNode>,
    ) -> Result<Self, DuplicateRecipeIdError> {
        // IDs of *all* nodes are unique, which means we can build a flat lookup
        // map for all recipes. This is also where we enforce uniqueness
        let mut nodes_by_id = IndexMap::new();
        let mut new = Self {
            tree,
            nodes_by_id: IndexMap::default(),
        };
        for (lookup_key, node) in new.iter() {
            let evicted = nodes_by_id.insert(node.id().clone(), lookup_key);
            if evicted.is_some() {
                return Err(DuplicateRecipeIdError(node.id().clone()));
            }
        }
        new.nodes_by_id = nodes_by_id;
        Ok(new)
    }

    /// Get a flat iterator over all nodes in the tree, using depth first
    /// search. Each yielded item will include the lookup key to retrieve
    /// that item.
    fn iter(&self) -> impl Iterator<Item = (RecipeLookupKey, &RecipeNode)> {
        // We'll lean on the inner IndexMap iterator for the hard work. We just
        // keep a stack of all the branches we're iterating over

        struct Iter<'a> {
            stack: Vec<Values<'a, RecipeId, RecipeNode>>,
            path: Vec<&'a RecipeId>,
        }

        impl<'a> Iterator for Iter<'a> {
            type Item = (RecipeLookupKey, &'a RecipeNode);

            fn next(&mut self) -> Option<Self::Item> {
                while let Some(iter) = self.stack.last_mut() {
                    match iter.next() {
                        Some(node @ RecipeNode::Folder(folder)) => {
                            // Go down this branch next
                            self.path.push(&folder.id);
                            self.stack.push(folder.children.values());
                            return Some(((&self.path).into(), node));
                        }
                        Some(node @ RecipeNode::Recipe(recipe)) => {
                            let mut lookup_key: RecipeLookupKey =
                                (&self.path).into();
                            lookup_key.0.push(recipe.id.clone());
                            return Some((lookup_key, node));
                        }
                        None => {
                            self.stack.pop();
                            self.path.pop();
                        }
                    }
                }
                // We ran out of iteration :(
                None
            }
        }

        Iter {
            stack: vec![self.tree.values()],
            path: Vec::new(),
        }
    }
}

impl<'de> Deserialize<'de> for RecipeTree {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let tree: IndexMap<RecipeId, RecipeNode> =
            deserialize_id_map(deserializer)?;
        Self::new(tree).map_err(D::Error::custom)
    }
}
