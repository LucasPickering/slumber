use crate::{Expression, Identifier, Literal, TemplateChunk};
use indexmap::IndexMap;
use proptest::{
    collection,
    prelude::{Strategy, any},
    prop_oneof,
    sample::SizeRange,
};
use std::hash::Hash;

/// Join consecutive raw chunks in a generated template, to make it valid
pub fn join_raw(chunks: Vec<TemplateChunk>) -> Vec<TemplateChunk> {
    let len = chunks.len();
    chunks
        .into_iter()
        .fold(Vec::with_capacity(len), |mut chunks, chunk| {
            match (chunks.last_mut(), chunk) {
                // If previous and current are both raw, join them together
                (
                    Some(TemplateChunk::Raw(previous)),
                    TemplateChunk::Raw(current),
                ) => {
                    // The current string is inside an Arc so we can't push
                    // into it, we have to clone it out :(
                    let mut concat =
                        String::with_capacity(previous.len() + current.len());
                    concat.push_str(previous);
                    concat.push_str(&current);
                    *previous = concat.into();
                }
                (_, chunk) => chunks.push(chunk),
            }
            chunks
        })
}

/// Generate an arbitrary expression. This needs a manual implementation because
/// it's recursive. Actually implementing Arbitrary manually is a pain because
/// we need to name the generated Strategy type. Using a free function and
/// attaching it to the parent is much easier because we can just return
/// `impl Strategy`
pub fn expression_arbitrary() -> impl Strategy<Value = Expression> {
    // This has to be implemented manually because it's recursive
    // https://proptest-rs.github.io/proptest/proptest/tutorial/recursive.html

    use crate::Expression;
    const COLLECTION_SIZE: usize = 5;

    let leaf = prop_oneof![
        any::<Literal>().prop_map(Expression::Literal),
        any::<Identifier>().prop_map(Expression::Field),
    ];
    leaf.prop_recursive(5, 256, COLLECTION_SIZE as u32, |inner| {
        prop_oneof![
            // Define recursive cases
            collection::vec(inner.clone(), 0..COLLECTION_SIZE)
                .prop_map(Expression::Array),
        ]
    })
}

/// Create a strategy to generate `IndexMap`s containing keys and values drawn
/// from `key` and `value` respectively, and with a size within the given
/// range.
pub fn index_map<K: Strategy, V: Strategy>(
    key: K,
    value: V,
    size: impl Into<SizeRange>,
) -> impl Strategy<Value = IndexMap<K::Value, V::Value>>
where
    K::Value: Hash + Eq,
{
    // Just generate a hashmap, then convert it. Order is random anyway
    collection::hash_map(key, value, size)
        .prop_map(|map| map.into_iter().collect())
}
