//! Generate strings (and bytes) from user-written templates with dynamic data.
//! This engine is focused on rendering templates, and is generally agnostic of
//! its usage in the rest of the app. As such, there is no logic in here
//! relating to HTTP or other Slumber concepts.
//!
//! TODO update comment

mod cereal;
mod display;
mod error;
mod function;
mod parse;
mod render;

pub use error::TemplateError;
pub use function::{Arguments, FunctionOutput, TryFromValue};

use derive_more::{Deref, Display, derive::From};
use futures::future;
use indexmap::IndexMap;
#[cfg(test)]
use proptest::{arbitrary::any, strategy::Strategy};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, fmt::Debug, sync::Arc};

use crate::parse::{FALSE, NULL, TRUE};

/// TODO
/// TODO rename to Context
pub trait TemplateContext: Sized + Send + Sync {
    /// TODO
    fn get(
        &self,
        identifier: &Identifier,
    ) -> impl Future<Output = Result<Value, TemplateError>> + Send;

    /// TODO
    fn call(
        &self,
        function_name: &Identifier,
        arguments: Arguments<'_, Self>,
    ) -> impl Future<Output = Result<Value, TemplateError>> + Send;
}

/// A parsed template, which can contain raw and/or templated content. The
/// string is parsed during creation to identify template keys, hence the
/// immutability.
///
/// The original string is *not* stored. To recover the source string, use the
/// [Display] implementation.
///
/// Invariants:
/// - Two templates with the same source string will have the same set of
///   chunks, and vice versa
/// - No two raw segments will ever be consecutive
#[derive(Clone, Debug, Default, PartialEq)]
#[cfg_attr(test, derive(proptest_derive::Arbitrary))]
pub struct Template {
    /// Pre-parsed chunks of the template. For raw chunks we store the
    /// presentation text (which is not necessarily the source text, as escape
    /// sequences will be eliminated). For keys, just store the needed
    /// metadata.
    #[cfg_attr(
        test,
        proptest(strategy = "any::<Vec<TemplateChunk>>().prop_map(join_raw)")
    )]
    chunks: Vec<TemplateChunk>,
}

impl Template {
    /// Create a new template from a raw string, without parsing it at all.
    /// Useful when importing from external formats where the string isn't
    /// expected to be a valid Slumber template
    pub fn raw(template: String) -> Template {
        let chunks = if template.is_empty() {
            vec![]
        } else {
            // This may seem too easy, but the hard part comes during
            // stringification, when we need to add backslashes to get the
            // string to parse correctly later
            vec![TemplateChunk::Raw(template.into())]
        };
        Self { chunks }
    }

    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }

    /// Render the template using values from the given context. If any chunk
    /// failed to render, return an error. The template is rendered as bytes,
    /// meaning it can safely render to non-UTF-8 content. Use
    /// [Self::render_string] if you want the bytes converted to a string.
    pub async fn render_bytes<Ctx: TemplateContext>(
        &self,
        context: &Ctx,
    ) -> Result<Vec<u8>, TemplateError> {
        let chunks = self.render_chunks(context).await;
        // TODO optimize this:
        // - Pre-allocate vec with capacity
        // - Avoid reallocation where possible for single-chunk results
        chunks.into_iter().try_fold(Vec::new(), |mut acc, chunk| {
            match chunk {
                RenderedChunk::Raw(s) => acc.extend(s.as_bytes()),
                RenderedChunk::Rendered(Value::Bytes(bytes)) => {
                    acc.extend(bytes)
                }
                RenderedChunk::Rendered(value) => acc.extend(
                    value
                        .try_into_string()
                        .expect("TODO remove unwrap")
                        .into_bytes(),
                ),
                RenderedChunk::Error(error) => return Err(error),
            }
            Ok(acc)
        })
    }

    /// Render the template using values from the given context. If any chunk
    /// failed to render, return an error. The rendered template will be
    /// converted from raw bytes to UTF-8. If it is not valid UTF-8, return an
    /// error.
    pub async fn render_string<Ctx: TemplateContext>(
        &self,
        context: &Ctx,
    ) -> Result<String, TemplateError> {
        let bytes = self.render_bytes(context).await?;
        String::from_utf8(bytes).map_err(TemplateError::other)
    }

    /// Render the template using values from the given context, returning the
    /// individual rendered chunks rather than stitching them together into a
    /// string. If any individual chunk fails to render, its error will be
    /// returned inline as [RenderedChunk::Error] and the rest of the template
    /// will still be rendered.
    pub async fn render_chunks<Ctx: TemplateContext>(
        &self,
        context: &Ctx,
    ) -> Vec<RenderedChunk> {
        // Map over each parsed chunk, and render the expressions into values.
        // because raw text uses Arc and expressions just contain metadata
        // The raw text chunks will be mapped 1:1. This clone is pretty cheap
        let futures = self.chunks.iter().map(|chunk| async move {
            match chunk {
                TemplateChunk::Raw(text) => {
                    RenderedChunk::Raw(Arc::clone(text))
                }
                TemplateChunk::Expression(expression) => expression
                    .render(context)
                    .await
                    .map_or_else(RenderedChunk::Error, RenderedChunk::Rendered),
            }
        });

        // Concurrency!
        future::join_all(futures).await
    }
}

#[cfg(any(test, feature = "test"))]
impl From<&str> for Template {
    fn from(value: &str) -> Self {
        value.parse().unwrap()
    }
}

#[cfg(any(test, feature = "test"))]
impl From<String> for Template {
    fn from(value: String) -> Self {
        value.as_str().into()
    }
}

#[cfg(any(test, feature = "test"))]
impl<const N: usize> From<[TemplateChunk; N]> for Template {
    fn from(chunks: [TemplateChunk; N]) -> Self {
        Self {
            chunks: chunks.into(),
        }
    }
}

/// A parsed piece of a template. After parsing, each chunk is either raw text
/// or a parsed key, ready to be rendered.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(test, derive(proptest_derive::Arbitrary))]
pub enum TemplateChunk {
    /// Raw unprocessed text, i.e. something **outside** the `{{ }}`. This is
    /// stored in an `Arc` so we can share cheaply in each render without
    /// having to clone text. This works because templates are immutable. Any
    /// non-empty string is a valid raw chunk. This text represents what the
    /// user wants to see, i.e. it does *not* including any escape chars.
    Raw(
        #[cfg_attr(test, proptest(strategy = "\".+\".prop_map(String::into)"))]
        Arc<str>,
    ),
    /// Dynamic expression to be computed at render time
    Expression(
        #[cfg_attr(test, proptest(strategy = "expression_arbitrary()"))]
        Expression,
    ),
}

#[cfg(test)]
impl From<Expression> for TemplateChunk {
    fn from(expression: Expression) -> Self {
        Self::Expression(expression)
    }
}

/// TODO
#[derive(Clone, Debug, PartialEq)]
pub enum Expression {
    /// TODO
    Literal(Literal),
    /// TODO
    Field(Identifier),
    /// Array literal: `[1, "hello", f()]`
    Array(Vec<Self>),
    /// Call to a plain function (**not** a filter)
    Call(FunctionCall),
    /// TODO update comment
    /// Data piped through a filter: `name | trim()`
    ///
    /// The left-hand side can be any expression, but the right-hand side must
    /// be a function call to a filter function. Filter functions are
    /// specifically defined to take the input "stdin" data as extra input.
    Pipe {
        expression: Box<Self>,
        call: FunctionCall,
    },
}

/// Literal primitive value
#[derive(Clone, Debug, From, PartialEq)]
#[cfg_attr(test, derive(proptest_derive::Arbitrary))]
pub enum Literal {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
}

impl From<&str> for Literal {
    fn from(value: &str) -> Self {
        Self::String(value.to_owned())
    }
}

/// Function call in a template expression: `f(true, 0, kwarg0="hello")`
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(test, derive(proptest_derive::Arbitrary))]
pub struct FunctionCall {
    function: Identifier,
    /// Positional arguments
    #[cfg_attr(
        test,
        proptest(
            strategy = "proptest::collection::vec(expression_arbitrary(), 0..=3)"
        )
    )]
    position: Vec<Expression>,
    /// Keyword arguments
    #[cfg_attr(
        test,
        proptest(
            strategy = "proptest::collection::hash_map(Identifier::arbitrary(), expression_arbitrary(), 0..=3)"
        )
    )]
    // TODO make this IndexMap to preserve order
    keyword: HashMap<Identifier, Expression>,
}

/// An identifier that can be used in a template key. A valid identifier is
/// any non-empty string that contains only alphanumeric characters, `-`, or
/// `_`.
///
/// Construct via [FromStr](std::str::FromStr)
#[derive(Clone, Debug, Deref, Default, Display, Eq, Hash, PartialEq)]
#[cfg_attr(test, derive(proptest_derive::Arbitrary))]
pub struct Identifier(
    #[cfg_attr(test, proptest(regex = "[a-zA-Z0-9-_]+"))] String,
);

/// A shortcut for creating identifiers from static strings. Since the string
/// is defined in code we're assuming it's valid.
impl From<&'static str> for Identifier {
    fn from(value: &'static str) -> Self {
        Self(value.parse().unwrap())
    }
}

/// TODO
#[derive(Clone, Debug, From, PartialEq, Serialize, Deserialize)]
pub enum Value {
    // TODO use Arc to make these cheaper to clone?
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Bytes(Vec<u8>), // TODO Bytes instead?
    Array(Vec<Self>),
    Object(IndexMap<String, Self>),
}

impl Value {
    /// Convert this value to a boolean, according to its truthiness.
    /// Truthiness/falsiness is defined for each type as:
    /// - `null` - `false`
    /// - `bool` - Own value
    /// - `int` - `false` if zero
    /// - `float` - `false` if zero
    /// - `string` - `false` if empty
    /// - `bytes` - `false` if empty
    /// - `array` - `false` if empty
    /// - `object` - `false` if empty
    /// These correspond to the truthiness rules from Python.
    pub fn to_bool(&self) -> bool {
        match self {
            Self::Null => false,
            Self::Bool(b) => *b,
            Self::Int(i) => *i != 0,
            Self::Float(f) => *f != 0.0,
            Self::String(s) => !s.is_empty(),
            Self::Bytes(bytes) => !bytes.is_empty(),
            Self::Array(array) => !array.is_empty(),
            Self::Object(object) => !object.is_empty(),
        }
    }

    /// Attempt to convert this value to a string. This can fail only if the
    /// value contains non-UTF-8 bytes, or if it is a collection that contains
    /// non-UTF-8 bytes.
    pub fn try_into_string(self) -> Result<String, TemplateError> {
        match self {
            Self::Null => Ok(NULL.into()),
            Self::Bool(false) => Ok(FALSE.into()),
            Self::Bool(true) => Ok(TRUE.into()),
            Self::Int(i) => Ok(i.to_string().into()),
            Self::Float(f) => Ok(f.to_string().into()),
            Self::String(s) => Ok(s),
            Self::Bytes(bytes) => {
                String::from_utf8(bytes).map_err(TemplateError::from)
            }
            // Use the display impl
            Self::Array(_) | Self::Object(_) => Ok(self.to_string()),
        }
    }
}

/// A piece of a rendered template string. A collection of chunks collectively
/// constitutes a rendered string when displayed contiguously.
#[derive(Debug)]
pub enum RenderedChunk {
    /// Raw unprocessed text, i.e. something **outside** the `{{ }}`. This is
    /// stored in an `Arc` so we can reference the text in the parsed input
    /// without having to clone it.
    Raw(Arc<str>),
    /// Outcome of rendering a template key
    Rendered(Value),
    /// An error occurred while rendering a template key
    Error(TemplateError),
}

#[cfg(test)]
impl PartialEq for RenderedChunk {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Raw(raw1), Self::Raw(raw2)) => raw1 == raw2,
            (Self::Rendered(value1), Self::Rendered(value2)) => {
                value1 == value2
            }
            (Self::Error(error1), Self::Error(error2)) => {
                // TemplateError doesn't have a PartialEq impl, so we have to
                // do a string comparison.
                error1.to_string() == error2.to_string()
            }
            _ => false,
        }
    }
}

/// Join consecutive raw chunks in a generated template, to make it valid
#[cfg(test)]
fn join_raw(chunks: Vec<TemplateChunk>) -> Vec<TemplateChunk> {
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
#[cfg(test)]
fn expression_arbitrary() -> impl Strategy<Value = Expression> {
    // This has to be implemented manually because it's recursive
    // https://proptest-rs.github.io/proptest/proptest/tutorial/recursive.html
    use proptest::{collection, prop_oneof};

    let leaf = prop_oneof![
        any::<Literal>().prop_map(Expression::Literal),
        any::<Identifier>().prop_map(Expression::Field),
    ];
    const COLLECTION_SIZE: usize = 5;
    leaf.prop_recursive(5, 256, COLLECTION_SIZE as u32, |inner| {
        prop_oneof![
            // Define recursive cases
            collection::vec(inner.clone(), 0..COLLECTION_SIZE)
                .prop_map(Expression::Array),
        ]
    })
}
