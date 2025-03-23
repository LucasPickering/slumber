//! Recipe/folder tree structure

use crate::collection::{
    Folder, HasId, Recipe, RecipeId, cereal::deserialize_id_map,
};
use anyhow::anyhow;
use derive_more::From;
use indexmap::{IndexMap, map::Values};
use serde::{Deserialize, Deserializer, Serialize, de::Error};
use strum::EnumDiscriminants;
use thiserror::Error;

/// A folder/recipe tree. This is exactly what the user inputs in their
/// collection file. IDs in this tree are **globally* unique, meaning no two
/// nodes can have the same ID anywhere in the tree, even between folders and
/// recipes. This is a mild restriction on the user that makes implementing a
/// lot simpler. In reality it's unlikely they would want to give two things
/// the same ID anyway.
#[derive(derive_more::Debug, Default)]
#[cfg_attr(any(test, feature = "test"), derive(PartialEq))]
pub struct RecipeTree {
    /// Tree structure storing all the folder/recipe data
    tree: IndexMap<RecipeId, RecipeNode>,
    /// A flattened version of the tree, with each ID pointing to its path in
    /// the tree. This is possible because the IDs are globally unique. It is
    /// an invariant that every lookup key in this map is valid, therefore it's
    /// safe to panic if one is found to be invalid.
    #[debug(skip)] // It's big and useless
    nodes_by_id: IndexMap<RecipeId, RecipeLookupKey>,
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
    pub fn new(
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

    /// Get a recipe/folder's tree lookup key by is unique ID
    pub fn get_lookup_key(&self, id: &RecipeId) -> Option<&RecipeLookupKey> {
        self.nodes_by_id.get(id)
    }

    /// Get a folder/recipe by ID
    pub fn get(&self, id: &RecipeId) -> Option<&RecipeNode> {
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
    pub fn try_get(&self, id: &RecipeId) -> anyhow::Result<&RecipeNode> {
        self.get(id)
            .ok_or_else(|| anyhow!("No recipe node with ID `{}`", id,))
    }

    /// Get a **recipe** by ID. If the ID isn't in the tree, or points to a
    /// folder, return `None`
    pub fn get_recipe(&self, id: &RecipeId) -> Option<&Recipe> {
        self.get(id).and_then(RecipeNode::recipe)
    }

    /// Get a **recipe** by ID. If the ID isn't in the tree, or points to a
    /// folder, return an error that can be presented to the user
    pub fn try_get_recipe(&self, id: &RecipeId) -> anyhow::Result<&Recipe> {
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
    pub fn iter(&self) -> impl Iterator<Item = (RecipeLookupKey, &RecipeNode)> {
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

impl Serialize for RecipeTree {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.tree.serialize(serializer)
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

#[cfg(any(test, feature = "test"))]
impl From<IndexMap<RecipeId, Recipe>> for RecipeTree {
    fn from(value: IndexMap<RecipeId, Recipe>) -> Self {
        value
            .into_iter()
            .map(|(id, recipe)| (id, RecipeNode::Recipe(recipe)))
            .collect::<IndexMap<_, _>>()
            .into()
    }
}

#[cfg(any(test, feature = "test"))]
impl From<IndexMap<RecipeId, RecipeNode>> for RecipeTree {
    fn from(tree: IndexMap<RecipeId, RecipeNode>) -> Self {
        Self::new(tree).unwrap()
    }
}

impl RecipeNode {
    pub fn name(&self) -> &str {
        match self {
            Self::Folder(folder) => folder.name(),
            Self::Recipe(recipe) => recipe.name(),
        }
    }

    /// If this node is a recipe, return it. Otherwise return `None`
    pub fn recipe(&self) -> Option<&Recipe> {
        match self {
            Self::Recipe(recipe) => Some(recipe),
            Self::Folder(_) => None,
        }
    }

    /// If this node is a folder, return it. Otherwise return `None`
    pub fn folder(&self) -> Option<&Folder> {
        match self {
            Self::Recipe(_) => None,
            Self::Folder(folder) => Some(folder),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{Factory, by_id};
    use indexmap::indexmap;
    use itertools::Itertools;
    use rstest::{fixture, rstest};
    use serde_yaml::{
        Value,
        value::{Tag, TaggedValue},
    };
    use slumber_util::assert_err;

    impl<const N: usize> From<[&str; N]> for RecipeLookupKey {
        fn from(value: [&str; N]) -> Self {
            value.into_iter().map(RecipeId::from).collect_vec().into()
        }
    }

    /// Shorthand!
    fn id(s: &str) -> RecipeId {
        s.into()
    }

    /// Build a YAML mapping
    fn mapping<const N: usize>(items: [(&str, Value); N]) -> Value {
        Value::Mapping(
            items
                .into_iter()
                .map(|(key, value)| (Value::from(key), value))
                .collect(),
        )
    }

    /// Build a YAML mapping with a variant tag
    fn tagged_mapping<const N: usize>(
        tag: &str,
        items: [(&str, Value); N],
    ) -> Value {
        Value::Tagged(Box::new(TaggedValue {
            tag: Tag::new(tag),
            value: mapping(items),
        }))
    }

    #[fixture]
    fn tree() -> IndexMap<RecipeId, RecipeNode> {
        by_id([
            Recipe {
                id: id("r1"),
                ..Recipe::factory(())
            }
            .into(),
            Folder {
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

    /// Deserializing with a duplicate ID anywhere in the tree should fail
    #[rstest]
    #[case::anywhere(
        // Two requests share an ID
        mapping([
            (
                "dupe",
                tagged_mapping(
                    "!request",
                    [("method", "GET".into()), ("url", "url".into())],
                ),
            ),
            (
                "f1",
                tagged_mapping(
                    "!folder",
                    [(
                        "requests",
                        mapping([(
                            "dupe",
                            tagged_mapping(
                                "!request",
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
                tagged_mapping(
                    "!folder",
                    [(
                        "requests",
                        mapping([("dupe", tagged_mapping("!folder", []))]),
                    )],
                ),
            ),
            ("dupe", tagged_mapping("!folder", [])),
        ])
    )]
    // Request + folder share an ID
    #[case::request_folder(
        mapping([
            (
                "f1",
                tagged_mapping(
                    "!folder",
                    [(
                        "requests",
                        tagged_mapping(
                            "!request",
                            [("dupe", tagged_mapping("!folder", []))],
                        ),
                    )],
                ),
            ),
            (
                "dupe",
                tagged_mapping(
                    "!request",
                    [("method", "GET".into()), ("url", "url".into())],
                ),
            ),
        ])
    )]
    fn test_duplicate_id(#[case] yaml_value: Value) {
        assert_err!(
            serde_yaml::from_value::<RecipeTree>(yaml_value),
            "Duplicate recipe/folder ID `dupe`"
        );
    }

    /// Test successful serialization/deserialization
    #[rstest]
    fn test_deserialization(tree: IndexMap<RecipeId, RecipeNode>) {
        // Manually create the ID map to make sure it's correct
        let tree = RecipeTree {
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

        // Create equivalent YAML
        let recipe_value: Value = tagged_mapping(
            "!request",
            [
                ("method", "GET".into()),
                ("url", "http://localhost/url".into()),
            ],
        );
        let yaml = mapping([
            ("r1", recipe_value.clone()),
            (
                "f1",
                tagged_mapping(
                    "!folder",
                    [(
                        "requests",
                        mapping([
                            (
                                "f2",
                                tagged_mapping(
                                    "!folder",
                                    [(
                                        "requests",
                                        mapping([("r2", recipe_value.clone())]),
                                    )],
                                ),
                            ),
                            ("r3", recipe_value.clone()),
                        ]),
                    )],
                ),
            ),
            ("r4", recipe_value.clone()),
        ]);

        assert_eq!(
            serde_yaml::from_value::<RecipeTree>(yaml).unwrap(),
            tree,
            "Deserialization failed"
        );
    }
}
