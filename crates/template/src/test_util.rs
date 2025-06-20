use crate::{Expression, FunctionCall, Identifier, Literal, TemplateChunk};
use bytes::Bytes;
use indexmap::IndexMap;
use proptest::{
    collection,
    prelude::{Arbitrary, Strategy, any},
    prop_oneof,
    sample::SizeRange,
};
use std::hash::Hash;

/// Generate an arbitrary byte array
pub fn bytes() -> impl Strategy<Value = Bytes> {
    any::<Vec<u8>>().prop_map(Bytes::from)
}

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

    let leaf = prop_oneof![
        any::<Literal>().prop_map(Expression::Literal),
        any::<Identifier>().prop_map(Expression::Field),
    ];
    leaf.prop_recursive(2, 10, 2, |inner| {
        prop_oneof![
            // Define recursive cases
            collection::vec(inner.clone(), 0..=2).prop_map(Expression::Array),
            // Generate a function call
            (
                Identifier::arbitrary(),
                collection::vec(inner.clone(), 0..=1),
                collection::hash_map(
                    Identifier::arbitrary(),
                    inner.clone(),
                    0..=1
                )
            )
                .prop_map(|(function, position, keyword)| {
                    Expression::Call(FunctionCall {
                        function,
                        position,
                        keyword: keyword.into_iter().collect(),
                    })
                }),
            // Being lazy and skipping pipe expressions
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
