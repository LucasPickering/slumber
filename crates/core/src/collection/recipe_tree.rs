//! Recipe/folder tree structure

use crate::{
    collection::{Folder, Recipe, RecipeId},
    render::Procedure,
};
use anyhow::anyhow;
use derive_more::From;
use indexmap::{IndexMap, map::Values};
use serde::{Deserialize, Deserializer, Serialize, de::Error};
use slumber_util::{HasId, deserialize_id_map};
use strum::EnumDiscriminants;
use thiserror::Error;

/// A folder/recipe tree. This is exactly what the user inputs in their
/// collection file. IDs in this tree are **globally* unique, meaning no two
/// nodes can have the same ID anywhere in the tree, even between folders and
/// recipes. This is a mild restriction on the user that makes implementing a
/// lot simpler. In reality it's unlikely they would want to give two things
/// the same ID anyway.
#[derive(derive_more::Debug)]
#[cfg_attr(any(test, feature = "test"), derive(PartialEq))]
pub struct RecipeTree<P = Procedure> {
    /// Tree structure storing all the folder/recipe data
    tree: IndexMap<RecipeId, RecipeNode<P>>,
    /// A flattened version of the tree, with each ID pointing to its path in
    /// the tree. This is possible because the IDs are globally unique. It is
    /// an invariant that every lookup key in this map is valid, therefore it's
    /// safe to panic if one is found to be invalid.
    #[debug(skip)] // It's big and useless
    nodes_by_id: IndexMap<RecipeId, RecipeLookupKey>,
}

impl<P> RecipeTree<P> {
    /// Create a new tree. If there are *any* duplicate IDs in the tree, the
    /// duplicate ID will be returned as an `Err`.
    pub fn new(
        tree: IndexMap<RecipeId, RecipeNode<P>>,
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

    /// Get a recipe/folder's tree lookup key by is unique ID
    pub fn get_lookup_key(&self, id: &RecipeId) -> Option<&RecipeLookupKey> {
        self.nodes_by_id.get(id)
    }

    /// Get a folder/recipe by ID
    pub fn get(&self, id: &RecipeId) -> Option<&RecipeNode<P>> {
        let lookup_key = self.nodes_by_id.get(id)?;
        let mut nodes = &self.tree;
        for (depth, step) in lookup_key.0.iter().enumerate() {
            let is_last = depth == lookup_key.0.len() - 1;
            let node = nodes.get(step).unwrap_or_else(|| {
                panic!("Lookup key {lookup_key:?} does not point to a node")
            });
            if is_last {
                return Some(node);
            }
            match node {
                RecipeNode::Folder(folder) => nodes = &folder.children,
                RecipeNode::Recipe(recipe) => panic!(
                    "Lookup key {lookup_key:?} attempts to traverse through \
                    recipe node `{}`",
                    recipe.id
                ),
            }
        }
        None
    }

    /// Get a folder/recipe by ID. Return an error if the ID isn't in the tree
    pub fn try_get(&self, id: &RecipeId) -> anyhow::Result<&RecipeNode<P>> {
        self.get(id)
            .ok_or_else(|| anyhow!("No recipe node with ID `{}`", id,))
    }

    /// Get a **recipe** by ID. If the ID isn't in the tree, or points to a
    /// folder, return `None`
    pub fn get_recipe(&self, id: &RecipeId) -> Option<&Recipe<P>> {
        self.get(id).and_then(RecipeNode::recipe)
    }

    /// Get a **recipe** by ID. If the ID isn't in the tree, or points to a
    /// folder, return an error that can be presented to the user
    pub fn try_get_recipe(&self, id: &RecipeId) -> anyhow::Result<&Recipe<P>> {
        self.get_recipe(id)
            .ok_or_else(|| anyhow!("No recipe with ID `{}`", id,))
    }

    /// Get all **recipe** IDs in the tree. Useful for printing a list to the
    /// user
    pub fn recipe_ids(&self) -> impl Iterator<Item = &RecipeId> {
        self.nodes_by_id
            .keys()
            .filter(|id| self.get_recipe(id).is_some())
    }

    /// Get a flat iterator over all nodes in the tree, using depth first
    /// search. Each yielded item will include the lookup key to retrieve
    /// that item.
    pub fn iter(
        &self,
    ) -> impl Iterator<Item = (RecipeLookupKey, &RecipeNode<P>)> {
        // We'll lean on the inner IndexMap iterator for the hard work. We just
        // keep a stack of all the branches we're iterating over

        struct Iter<'a, P> {
            stack: Vec<Values<'a, RecipeId, RecipeNode<P>>>,
            path: Vec<&'a RecipeId>,
        }

        impl<'a, P> Iterator for Iter<'a, P> {
            type Item = (RecipeLookupKey, &'a RecipeNode<P>);

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

    /// Get the inner map of nodes
    pub fn into_map(self) -> IndexMap<RecipeId, RecipeNode<P>> {
        self.tree
    }
}

// No bound on P
impl<P> Default for RecipeTree<P> {
    fn default() -> Self {
        Self {
            tree: Default::default(),
            nodes_by_id: Default::default(),
        }
    }
}

/// A path into the recipe tree. Every constructed path is assumed to be valid,
/// which must be enforced by the creator.
#[derive(Clone, Debug, From, Eq, Hash, PartialEq)]
pub struct RecipeLookupKey(Vec<RecipeId>);

impl RecipeLookupKey {
    /// How many nodes are above us in the tree?
    pub fn depth(&self) -> usize {
        self.0.len() - 1
    }

    /// Get all parent IDs, starting at the root
    pub fn ancestors(&self) -> &[RecipeId] {
        &self.0[0..self.0.len() - 1]
    }
}

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
#[derive(Debug, From, Serialize, Deserialize, EnumDiscriminants)]
#[strum_discriminants(name(RecipeNodeType))]
#[cfg_attr(any(test, feature = "test"), derive(PartialEq))]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    deny_unknown_fields,
    bound(deserialize = "P: Default + Deserialize<'de>")
)]
pub enum RecipeNode<P = Procedure> {
    Folder(Folder<P>),
    /// Rename this variant to match the `requests` field in the root and
    /// folders
    #[serde(rename = "request")]
    Recipe(Recipe<P>),
}

/// Error returned when attempting to build a [RecipeTree] with a duplicate
/// recipe ID. IDs are unique throughout the entire tree.
#[derive(Debug, Error)]
#[error(
    "Duplicate recipe/folder ID `{0}`; \
    recipe/folder IDs must be globally unique"
)]
pub struct DuplicateRecipeIdError(RecipeId);

impl<P> Serialize for RecipeTree<P>
where
    P: Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.tree.serialize(serializer)
    }
}

impl<'de, P> Deserialize<'de> for RecipeTree<P>
where
    P: Default + Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let tree: IndexMap<RecipeId, RecipeNode<P>> =
            deserialize_id_map(deserializer)?;
        Self::new(tree).map_err(D::Error::custom)
    }
}

#[cfg(any(test, feature = "test"))]
impl<P> From<IndexMap<RecipeId, Recipe<P>>> for RecipeTree<P> {
    fn from(value: IndexMap<RecipeId, Recipe<P>>) -> Self {
        value
            .into_iter()
            .map(|(id, recipe)| (id, RecipeNode::Recipe(recipe)))
            .collect::<IndexMap<_, _>>()
            .into()
    }
}

#[cfg(any(test, feature = "test"))]
impl<P> From<IndexMap<RecipeId, RecipeNode<P>>> for RecipeTree<P> {
    fn from(tree: IndexMap<RecipeId, RecipeNode<P>>) -> Self {
        Self::new(tree).unwrap()
    }
}

impl<P> RecipeNode<P> {
    pub fn name(&self) -> &str {
        match self {
            Self::Folder(folder) => folder.name(),
            Self::Recipe(recipe) => recipe.name(),
        }
    }

    /// If this node is a recipe, return it. Otherwise return `None`
    pub fn recipe(&self) -> Option<&Recipe<P>> {
        match self {
            Self::Recipe(recipe) => Some(recipe),
            Self::Folder(_) => None,
        }
    }

    /// If this node is a folder, return it. Otherwise return `None`
    pub fn folder(&self) -> Option<&Folder<P>> {
        match self {
            Self::Recipe(_) => None,
            Self::Folder(folder) => Some(folder),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::by_id;
    use indexmap::indexmap;
    use itertools::Itertools;
    use petitscript::{Value, value::Object};
    use rstest::{fixture, rstest};
    use slumber_util::{Factory, assert_err};

    /// Test flat iteration over the tree
    #[rstest]
    fn test_iter(tree: IndexMap<RecipeId, RecipeNode>) {
        let tree = RecipeTree::new(tree).unwrap();
        let expected: Vec<(RecipeLookupKey, RecipeId)> = vec![
            (["r1"].into(), id("r1")),
            (["f1"].into(), id("f1")),
            (["f1", "f2"].into(), id("f2")),
            (["f1", "f2", "r2"].into(), id("r2")),
            (["f1", "r3"].into(), id("r3")),
            (["r4"].into(), id("r4")),
        ];

        // Just compare lookup keys and IDs, to keep it simple
        assert_eq!(
            tree.iter()
                .map(|(key, node)| (key, node.id().clone()))
                .collect_vec(),
            expected
        );
    }

    /// Test successful serialization/deserialization
    #[rstest]
    fn test_deserialization(tree: IndexMap<RecipeId, RecipeNode>) {
        // Manually create the ID map to make sure it's correct
        let tree = RecipeTree::<Procedure> {
            tree,
            nodes_by_id: indexmap! {
                id("r1") => ["r1"].into(),
                id("f1") => ["f1"].into(),
                id("f2") => ["f1", "f2"].into(),
                id("r2") => ["f1", "f2", "r2"].into(),
                id("r3") => ["f1", "r3"].into(),
                id("r4") => ["r4"].into(),
            },
        };

        // Create equivalent PS
        let recipe_value: Value = recipe([
            ("method", "GET".into()),
            ("url", "http://localhost/url".into()),
        ]);
        let value = mapping([
            ("r1", recipe_value.clone()),
            (
                "f1",
                folder([(
                    "requests",
                    mapping([
                        (
                            "f2",
                            folder([(
                                "requests",
                                mapping([("r2", recipe_value.clone())]),
                            )]),
                        ),
                        ("r3", recipe_value.clone()),
                    ]),
                )]),
            ),
            ("r4", recipe_value.clone()),
        ]);

        assert_eq!(
            petitscript::serde::from_value::<RecipeTree>(value).unwrap(),
            tree,
            "Deserialization failed"
        );
    }

    /// Deserializing with a duplicate ID anywhere in the tree should fail
    #[rstest]
    #[case::recipe(
        // Two requests share an ID
        mapping([
            ("dupe", recipe([("method", "GET".into()), ("url", "url".into())])),
            (
                "f1",
                folder(
                    [(
                        "requests",
                        mapping([(
                            "dupe",
                            recipe(
                                [
                                    ("method", "GET".into()),
                                    ("url", "url".into()),
                                ],
                            ),
                        )]),
                    )],
                ),
            ),
        ])
    )]
    // Two folders share an ID
    #[case::folder(
        mapping([
            (
                "f1",
                folder(
                    [(
                        "requests",
                        mapping([("dupe", folder([]))]),
                    )],
                ),
            ),
            ("dupe", folder([])),
        ])
    )]
    // Recipe + folder share an ID
    #[case::recipe_folder(
        mapping([
            (
                "f1",
                folder(
                    [(
                        "requests",
                        mapping([("dupe", folder([]))]),
                    )],
                ),
            ),
            (
                "dupe",
                recipe(
                    [("method", "GET".into()), ("url", "url".into())],
                ),
            ),
        ])
    )]
    fn test_duplicate_id(#[case] petit_value: Value) {
        assert_err!(
            petitscript::serde::from_value::<RecipeTree>(petit_value),
            "Duplicate recipe/folder ID `dupe`"
        );
    }

    impl<const N: usize> From<[&str; N]> for RecipeLookupKey {
        fn from(value: [&str; N]) -> Self {
            value.into_iter().map(RecipeId::from).collect_vec().into()
        }
    }

    /// Shorthand!
    fn id(s: &str) -> RecipeId {
        s.into()
    }

    /// Build a PS mapping
    fn mapping<const N: usize>(items: [(&str, Value); N]) -> Value {
        items.into()
    }

    /// Build a folder node
    fn folder<const N: usize>(fields: [(&str, Value); N]) -> Value {
        Object::new()
            .insert("type", "folder")
            .insert_all(fields.into())
            .into()
    }

    /// Build a recipe node
    fn recipe<const N: usize>(fields: [(&str, Value); N]) -> Value {
        Object::new()
            .insert("type", "request")
            .insert_all(fields.into())
            .into()
    }

    #[fixture]
    fn tree() -> IndexMap<RecipeId, RecipeNode> {
        by_id([
            Recipe {
                id: id("r1"),
                ..Recipe::factory(())
            }
            .into(),
            Folder::<Procedure> {
                id: id("f1"),
                children: by_id([
                    Folder {
                        id: id("f2"),
                        children: by_id([Recipe {
                            id: id("r2"),
                            ..Recipe::factory(())
                        }
                        .into()]),
                        ..Folder::factory(())
                    }
                    .into(),
                    Recipe {
                        id: id("r3"),
                        ..Recipe::factory(())
                    }
                    .into(),
                ]),
                ..Folder::factory(())
            }
            .into(),
            Recipe {
                id: id("r4"),
                ..Recipe::factory(())
            }
            .into(),
        ])
    }
}
