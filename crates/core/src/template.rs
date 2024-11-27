//! Generate strings (and bytes) from user-written templates with dynamic data

mod cereal;
mod error;
mod parse;
mod prompt;
mod render;

pub use error::{TemplateError, TemplateParseError, TriggeredRequestError};
pub use prompt::{Prompt, PromptChannel, Prompter, Select};

use crate::{
    collection::{Collection, Profile, ProfileId},
    db::CollectionDatabase,
    http::HttpEngine,
    template::parse::TemplateInputChunk,
};
use derive_more::Display;
use indexmap::IndexMap;
use mlua::Table;
#[cfg(test)]
use proptest::{arbitrary::any, strategy::Strategy};
use std::sync::Arc;
use tokio::sync::Mutex;

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
        proptest(
            strategy = "any::<Vec<TemplateInputChunk>>().prop_map(join_raw)"
        )
    )]
    chunks: Vec<TemplateInputChunk>,
}

/// A little container struct for all the data that the user can access via
/// templating. Unfortunately this has to own all data so templating can be
/// deferred into a task (tokio requires `'static` for spawned tasks). If this
/// becomes a bottleneck, we can `Arc` some stuff.
#[derive(Debug)]
pub struct TemplateContext {
    /// Entire request collection
    pub collection: Arc<Collection>,
    /// ID of the profile whose data should be used for rendering. Generally
    /// the caller should check the ID is valid before passing it, to
    /// provide a better error to the user if not.
    pub selected_profile: Option<ProfileId>,
    /// HTTP engine used to executed triggered sub-requests. This should only
    /// be populated if you actually want to trigger requests! In some cases
    /// you want renders to be idempotent, in which case you should pass
    /// `None`.
    pub http_engine: Option<HttpEngine>,
    /// Needed for accessing response bodies for chaining
    pub database: CollectionDatabase,
    /// Additional expression=value overrides passed directly from the user.
    /// The keys must be exact matches to the corresponding Lua expressions
    /// to be replaced.
    pub overrides: IndexMap<String, String>,
    /// A conduit to ask the user questions
    pub prompter: Box<dyn Prompter>,
    /// Internal state for a render group. This should always be initialized to
    /// `Default::default()`.
    pub state: RenderState,
}

impl Template {
    /// Create a new template from a raw string, without parsing it at all.
    /// Useful when importing from external formats where the string isn't
    /// expected to be a valid Slumber template
    pub fn raw(template: String) -> Self {
        let chunks = if template.is_empty() {
            vec![]
        } else {
            // This may seem too easy, but the hard part comes during
            // stringification, when we need to add backslashes to get the
            // string to parse correctly later
            vec![TemplateInputChunk::Raw(template.into())]
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

#[cfg(any(test, feature = "test"))]
impl From<serde_json::Value> for Template {
    fn from(value: serde_json::Value) -> Self {
        format!("{value:#}").into()
    }
}

impl TemplateContext {
    /// Get the selected profile from the collection
    pub fn profile(&self) -> Option<&Profile> {
        // Get the value from the profile
        let profile_id = self.selected_profile.as_ref()?;
        // Typically the caller should validate the ID before initializing
        // template context, but if it's invalid for some reason, return None
        self.collection.profiles.get(profile_id)
    }
}

/// A wrapper for any state that can change throughout the course of a render
/// group.
#[derive(Debug, Default)]
pub struct RenderState {
    /// A rendered+cached profile data for this render group. This should
    /// always be initialized to `Default::default()`.
    pub profile_data: Mutex<Option<Table>>,
}

/// A piece of a rendered template string. A collection of chunks collectively
/// constitutes a rendered string, and those chunks should be contiguous.
#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq))]
pub enum TemplateChunk {
    /// Raw unprocessed text, i.e. something **outside** the `{{ }}`. This is
    /// stored in an `Arc` so we can reference the text in the parsed input
    /// without having to clone it.
    Raw(Arc<String>),
    /// Outcome of rendering a template key
    Rendered {
        /// This is wrapped in `Arc` to de-duplicate large values derived from
        /// chains. When the same chain is used multiple times in a render
        /// group it gets deduplicated, meaning multiple render results would
        /// refer to the same data. In the vast majority of cases though we
        /// only ever have one pointer to this data. This is arguably a
        /// premature optimization, but it's very possible for a large chained
        /// body to be used twice, and we wouldn't want to duplicate that.
        value: Arc<Vec<u8>>,
        sensitive: bool,
    },
    /// An error occurred while rendering a template key
    Error(TemplateError),
}

#[cfg(test)]
impl TemplateChunk {
    /// Shorthand for creating a new raw chunk
    fn raw(value: &str) -> Self {
        Self::Raw(value.to_owned().into())
    }
}

/// A Lua expression embedded in a template. Any valid Lua expression can live
/// inside the `{{ }}` of a template.
///
/// **Note:** The contained expression is not actually parsed during template
/// parsing, so it is not necessarily valid. It won't be parsed and evaluated
/// until render time.
#[derive(Clone, Debug, Display, PartialEq)]
#[display("{source}")]
#[cfg_attr(test, derive(proptest_derive::Arbitrary))]
pub struct TemplateExpression {
    source: String,
}

impl TemplateExpression {
    fn new(source: String) -> Self {
        Self { source }
    }
}

#[cfg(any(test, feature = "test"))]
impl crate::test_util::Factory for TemplateContext {
    fn factory(_: ()) -> Self {
        use crate::test_util::TestPrompter;
        let database = CollectionDatabase::factory(());
        Self {
            collection: Default::default(),
            selected_profile: None,
            http_engine: None,
            database,
            overrides: IndexMap::new(),
            prompter: Box::<TestPrompter>::default(),
            state: Default::default(),
        }
    }
}

/// Join consecutive raw chunks in a generated template, to make it valid
#[cfg(test)]
fn join_raw(chunks: Vec<TemplateInputChunk>) -> Vec<TemplateInputChunk> {
    let len = chunks.len();
    chunks
        .into_iter()
        .fold(Vec::with_capacity(len), |mut chunks, chunk| {
            match (chunks.last_mut(), chunk) {
                // If previous and current are both raw, join them together
                (
                    Some(TemplateInputChunk::Raw(previous)),
                    TemplateInputChunk::Raw(current),
                ) => {
                    // The current string is inside an Arc so we can't push
                    // into it, we have to clone it out :(
                    let mut concat =
                        String::with_capacity(previous.len() + current.len());
                    concat.push_str(previous);
                    concat.push_str(&current);
                    *previous = Arc::new(concat)
                }
                (_, chunk) => chunks.push(chunk),
            }
            chunks
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        assert_err,
        collection::Profile,
        lua::LuaRenderer,
        test_util::{by_id, invalid_utf8, Factory},
    };
    use anyhow::anyhow;
    use indexmap::indexmap;
    use rstest::rstest;

    /// Test rendering non-UTF-8 data
    #[rstest]
    #[tokio::test]
    async fn test_render_binary(invalid_utf8: Template) {
        let renderer = LuaRenderer::factory(());
        assert_eq!(invalid_utf8.render(&renderer).await.unwrap(), b"\xc3\x28");
    }

    /// Test rendering non-UTF-8 data to string returns an error
    #[rstest]
    #[tokio::test]
    async fn test_render_invalid_utf8(invalid_utf8: Template) {
        let renderer = LuaRenderer::factory(());
        assert_err!(render!(invalid_utf8, renderer), "invalid utf-8");
    }

    /// Test rendering into individual chunks with complex unicode
    #[tokio::test]
    async fn test_render_chunks() {
        let context =
            profile_context(indexmap! { "user_id".into() => "🧡💛".into() });
        let renderer = LuaRenderer::factory(context);

        let chunks =
            Template::from("intro {{user_id}} 💚💙💜 {{unknown()}} outro")
                .render_chunks(&renderer)
                .await;
        assert_eq!(
            chunks,
            vec![
                TemplateChunk::raw("intro "),
                TemplateChunk::Rendered {
                    value: Arc::new("🧡💛".into()),
                    sensitive: false
                },
                // Each emoji is 4 bytes
                TemplateChunk::raw(" 💚💙💜 "),
                TemplateChunk::Error(TemplateError::Lua(
                    mlua::Error::external(anyhow!("TODO")).into()
                )),
                TemplateChunk::raw(" outro"),
            ]
        );
    }

    /// Tested rendering a template with escaped keys, which should be treated
    /// as raw text
    #[tokio::test]
    async fn test_render_escaped() {
        let context =
            profile_context(indexmap! { "user_id".into() => "user1".into() });
        let renderer = LuaRenderer::factory(context);
        let template = "user: {{user_id}} escaped: {_{user_id}}";
        assert_eq!(
            render!(template, renderer).unwrap(),
            "user: user1 escaped: {{user_id}}"
        );
    }

    /// Build a template context that only has simple profile data
    fn profile_context(data: IndexMap<String, Template>) -> TemplateContext {
        let profile = Profile {
            data,
            ..Profile::factory(())
        };
        let profile_id = profile.id.clone();
        TemplateContext {
            collection: Collection {
                profiles: by_id([profile]),
                ..Collection::factory(())
            }
            .into(),
            selected_profile: Some(profile_id),
            ..TemplateContext::factory(())
        }
    }

    /// Helper for rendering a template to a string
    macro_rules! render {
        ($template:expr, $renderer:expr) => {
            Template::from($template).render_string(&$renderer).await
        };
    }
    use render;
}
