//! Template rendering implementation

use crate::{
    collection::ChainOutputTrim,
    template::{TemplateChunk, TemplateError},
};
use std::sync::Arc;

/// Outcome of rendering a single chunk. This allows attaching some metadata to
/// the render.
#[derive(Clone, Debug)]
struct RenderedChunk {
    /// This is wrapped in `Arc` to de-duplicate large values derived from
    /// chains. When the same chain is used multiple times in a render group it
    /// gets deduplicated, meaning multiple render results would refer to the
    /// same data. In the vast majority of cases though we only ever have one
    /// pointer to this data.
    value: Arc<Vec<u8>>,
    sensitive: bool,
}

type TemplateResult = Result<RenderedChunk, TemplateError>;

impl From<TemplateResult> for TemplateChunk {
    fn from(result: TemplateResult) -> Self {
        match result {
            Ok(outcome) => Self::Rendered {
                value: outcome.value,
                sensitive: outcome.sensitive,
            },
            Err(error) => Self::Error(error),
        }
    }
}

/// State for a render group, which consists of one or more related renders
/// (e.g. all the template renders for a single recipe). This state is stored in
/// the template context.
#[derive(Debug, Default)]
pub struct RenderGroupState {
    // TODO how to cache multiple calls to the same fn?
}

impl ChainOutputTrim {
    /// Apply whitespace trimming to string values. If the value is not a valid
    /// string, no trimming is applied
    fn apply(self, value: Vec<u8>) -> Vec<u8> {
        // Theoretically we could strip whitespace-looking characters from
        // binary data, but if the whole thing isn't a valid string it doesn't
        // really make any sense to.
        let Ok(s) = std::str::from_utf8(&value) else {
            return value;
        };
        match self {
            Self::None => value,
            Self::Start => s.trim_start().into(),
            Self::End => s.trim_end().into(),
            Self::Both => s.trim().into(),
        }
    }
}
