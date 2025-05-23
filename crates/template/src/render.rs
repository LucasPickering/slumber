//! Template rendering implementation

use crate::{Template, TemplateError};
use bytes::Bytes;
use futures::future;
use std::sync::Arc;
use tracing::{error, instrument, trace, trace_span};

/// Outcome of rendering a single chunk. This allows attaching some metadata to
/// the render.
#[derive(Clone, Debug)]
struct RenderedChunk {
    value: Bytes,
    sensitive: bool,
}

type TemplateResult = Result<RenderedChunk, TemplateError>;
