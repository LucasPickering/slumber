//! Utilities for prop testing. This is separate from [crate::test_util] because
//! proptest code isn't currently exported.

use crate::{
    collection::{Folder, Recipe, RecipeId, RecipeNode, RecipeTree},
    template::Template,
};
use indexmap::IndexMap;
use proptest::{
    collection::{hash_map, vec},
    prelude::{any, Arbitrary, Strategy},
    sample::SizeRange,
};
use std::hash::Hash;

/// Strategy to generate an index map
pub fn index_map<K: Arbitrary + Hash + Eq, V: Arbitrary>(
    size: impl Into<SizeRange>,
) -> impl Strategy<Value = IndexMap<K, V>> {
    hash_map(any::<K>(), any::<V>(), size)
        .prop_map(|map| map.into_iter().collect::<IndexMap<_, _>>())
}

/// TODO
pub fn query_parameters() -> impl Strategy<Value = Vec<(String, Template)>> {
    vec((".+", any::<Template>()), 0..5)
}

/// TODO
pub fn recipe_tree() -> impl Strategy<Value = RecipeTree> {
    recipe_map(recipe_node()).prop_map(RecipeTree::from)
}

/// TODO
fn recipe_map(
    value: impl Strategy<Value = RecipeNode>,
) -> impl Strategy<Value = IndexMap<RecipeId, RecipeNode>> {
    hash_map(any::<RecipeId>(), value, 0..5)
        .prop_map(|map| map.into_iter().collect::<IndexMap<_, _>>())
}

/// TODO
fn recipe_node() -> impl Strategy<Value = RecipeNode> {
    let leaf = any::<Recipe>().prop_map(RecipeNode::Recipe);
    leaf.prop_recursive(0, 20, 10, |inner| {
        let id = any::<RecipeId>();
        let name = any::<Option<String>>();
        let children = recipe_map(inner);
        (id, name, children).prop_map(|(id, name, children)| {
            RecipeNode::Folder(Folder { id, name, children })
        })
    })
}
