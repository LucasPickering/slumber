//! Generate strings (and bytes) from user-written templates with dynamic data.
//! This engine is focused on rendering templates, and is generally agnostic of
//! its usage in the rest of the app. As such, there is no logic in here
//! relating to HTTP or other Slumber concepts.

mod cereal;
mod display;
mod error;
mod function;
mod parse;
mod render;
#[cfg(test)]
mod tests;

pub use error::TemplateError;
pub use function::{Kwargs, ViaSerde};

use crate::function::{BoxedFunction, Function, FunctionArgs, FunctionOutput};
use bytes::Bytes;
use derive_more::{Deref, Display};
use futures::future;
use indexmap::IndexMap;
#[cfg(test)]
use proptest::{arbitrary::any, strategy::Strategy};
use serde::{Deserialize, Serialize};
use std::{fmt::Debug, sync::Arc};

/// TODO
#[derive(Debug)]
pub struct TemplateEngine<Ctx> {
    /// Functions that produce values from arguments
    functions: IndexMap<&'static str, BoxedFunction<Ctx>>,
}

impl<Ctx: TemplateContext> TemplateEngine<Ctx> {
    /// Initialize a new template engine
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a function under the given name. If there is already a function
    /// with that name, it will be overwritten
    pub fn add_function<F, Args, Out>(
        &mut self,
        name: &'static str,
        function: F,
    ) where
        F: Function<Ctx, Args, Out>,
        // These bounds aren't strictly necessary because they're implied by
        // the Function bound, but they make type inference easier and provide
        // better type error messages
        Args: for<'a> FunctionArgs<'a, Ctx>,
        Out: FunctionOutput,
    {
        self.functions.insert(name, BoxedFunction::new(function));
    }

    /// Get a function by name
    fn get_function(
        &self,
        name: &str,
    ) -> Result<&BoxedFunction<Ctx>, TemplateError> {
        // TODO include help message in error
        self.functions
            .get(name)
            .ok_or_else(|| TemplateError::UnknownFunction {
                name: name.to_owned(),
            })
    }

    /// Render the template using values from the given context. If any chunk
    /// failed to render, return an error. The template is rendered as bytes,
    /// meaning it can safely render to non-UTF-8 content. Use
    /// [Self::render_string] if you want the bytes converted to a string.
    pub async fn render_bytes(
        &self,
        template: &Template,
        context: &Ctx,
    ) -> Result<Bytes, TemplateError> {
        let chunks = self.render_chunks(template, context).await;
        todo!()
    }

    /// Render the template using values from the given context. If any chunk
    /// failed to render, return an error. The rendered template will be
    /// converted from raw bytes to UTF-8. If it is not valid UTF-8, return an
    /// error.
    pub async fn render_string(
        &self,
        template: &Template,
        context: &Ctx,
    ) -> Result<String, TemplateError> {
        let chunks = self.render_chunks(template, context).await;
        todo!()
    }

    /// Render the template using values from the given context, returning the
    /// individual rendered chunks rather than stitching them together into a
    /// string. If any individual chunk fails to render, its error will be
    /// returned inline as [RenderedChunk::Error] and the rest of the template
    /// will still be rendered.
    pub async fn render_chunks(
        &self,
        template: &Template,
        context: &Ctx,
    ) -> Vec<RenderedChunk> {
        // Map over each parsed chunk, and render the expressions into values.
        // because raw text uses Arc and expressions just contain metadata
        // The raw text chunks will be mapped 1:1. This clone is pretty cheap
        let futures = template.chunks.iter().map(|chunk| async move {
            match chunk {
                TemplateChunk::Raw(text) => {
                    RenderedChunk::Raw(Arc::clone(text))
                }
                TemplateChunk::Expression(expression) => expression
                    .render(self, context)
                    .await
                    .map_or_else(RenderedChunk::Error, RenderedChunk::Rendered),
            }
        });

        // Concurrency!
        future::join_all(futures).await
    }
}

// Manual impl needed to avoid bound on Ctx
impl<Ctx> Default for TemplateEngine<Ctx> {
    fn default() -> Self {
        Self {
            functions: IndexMap::default(),
        }
    }
}

/// TODO
pub trait TemplateContext: Sized + Send + Sync {
    /// TODO
    fn get(
        &self,
        identifier: &Identifier,
        engine: &TemplateEngine<Self>,
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
    Expression(Expression),
}

/// TODO
#[derive(Clone, Debug, PartialEq)]
pub enum Expression {
    /// TODO
    Literal(Literal),
    /// Array literal: `[1, "hello", f()]`
    Array(Vec<Self>),
    /// TODO
    Field(Identifier),
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
#[derive(Clone, Debug, PartialEq)]
pub enum Literal {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
}

/// Function call in a template expression: `f(true, 0, kwarg0="hello")`
#[derive(Clone, Debug, PartialEq)]
pub struct FunctionCall {
    function: Identifier,
    arguments: Vec<Expression>,
    kwargs: IndexMap<Identifier, Expression>,
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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Value {
    // TODO use Arc to make these cheaper to clone?
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Bytes(Bytes),
    Array(Vec<Self>),
    Object(IndexMap<String, Self>),
}

/// A piece of a rendered template string. A collection of chunks collectively
/// constitutes a rendered string when displayed contiguously.
#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq))]
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
impl RenderedChunk {
    /// Shorthand for creating a new raw chunk
    fn raw(value: &str) -> Self {
        Self::Raw(value.to_owned().into())
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
