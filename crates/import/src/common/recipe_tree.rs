//! Recipe/folder tree structure

use crate::common::{Folder, Recipe, RecipeId, cereal::deserialize_id_map};
use indexmap::IndexMap;
use serde::{Deserialize, Deserializer, de::Error};
use std::collections::HashSet;
use thiserror::Error;

/// A folder/recipe tree. This is exactly what the user inputs in their
/// collection file. IDs in this tree are **globally* unique, meaning no two
/// nodes can have the same ID anywhere in the tree, even between folders and
/// recipes. This is a mild restriction on the user that makes implementing a
/// lot simpler. In reality it's unlikely they would want to give two things
/// the same ID anyway.
#[derive(Debug, Default)]
pub(crate) struct RecipeTree {
    tree: IndexMap<RecipeId, RecipeNode>,
}

/// A node in the recipe tree, either a folder or recipe
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[allow(clippy::large_enum_variant)]
pub(crate) enum RecipeNode {
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
pub(crate) struct DuplicateRecipeIdError(RecipeId);

impl RecipeTree {
    /// Create a new tree. If there are *any* duplicate IDs in the tree, the
    /// duplicate ID will be returned as an `Err`.
    pub(crate) fn new(
        tree: IndexMap<RecipeId, RecipeNode>,
    ) -> Result<Self, DuplicateRecipeIdError> {
        /// Ensure that all IDs in the map are globally unique
        fn check_ids(
            ids: &mut HashSet<RecipeId>,
            map: &IndexMap<RecipeId, RecipeNode>,
        ) -> Result<(), DuplicateRecipeIdError> {
            for (id, node) in map.iter() {
                let evicted = ids.insert(id.clone());
                if evicted {
                    return Err(DuplicateRecipeIdError(id.clone()));
                }

                // Recursion!!
                if let RecipeNode::Folder(folder) = node {
                    check_ids(ids, &folder.children)?;
                }
            }
            Ok(())
        }

        check_ids(&mut HashSet::new(), &tree)?;
        Ok(Self { tree })
    }

    /// Get the root map
    pub fn into_map(self) -> IndexMap<RecipeId, RecipeNode> {
        self.tree
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
