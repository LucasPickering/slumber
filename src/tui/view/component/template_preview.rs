use crate::{
    config::ProfileId,
    template::{TemplateChunk, TemplateString},
    tui::{
        message::Message,
        view::{
            component::{Draw, DrawContext},
            theme::Theme,
            util::ToTui,
        },
    },
};
use ratatui::{
    prelude::Rect,
    text::{Line, Span, Text},
    widgets::Paragraph,
};
use std::{
    ops::Deref,
    sync::{Arc, OnceLock},
};

/// A preview of a template string, which can show either the raw text or the
/// rendered version. This switch is stored in render context, so it can be
/// changed globally.
#[derive(Debug)]
pub struct TemplatePreview {
    template: TemplateString,
    /// Rendered chunks. On init we send a message which will trigger a task to
    /// start the render. When the task is done, it'll dump its result back
    /// here.
    chunks: Arc<OnceLock<Vec<TemplateChunk>>>,
}

impl TemplatePreview {
    /// Create a new template preview. This will spawn a background task to
    /// render the template. Profile ID defines which profile to use for the
    /// render.
    pub fn new(
        context: &DrawContext,
        template: TemplateString,
        profile_id: Option<ProfileId>,
    ) -> Self {
        // Tell the controller to start rendering the preview, and it'll store
        // it back here when done
        let lock = Arc::new(OnceLock::new());
        context.messages_tx.send(Message::TemplatePreview {
            template: template.clone(), // If this is a bottleneck we can Arc it
            profile_id,
            destination: Arc::clone(&lock),
        });

        Self {
            template,
            chunks: lock,
        }
    }
}

impl ToTui for TemplatePreview {
    type Output<'this> = Text<'this>
    where
        Self: 'this;

    fn to_tui(&self, context: &DrawContext) -> Self::Output<'_> {
        // The raw template string
        let raw = self.template.deref();

        if context.preview_templates {
            // If the preview render is ready, show it. Otherwise fall back to
            // the raw
            match self.chunks.get() {
                Some(chunks) => {
                    Line::from(stitch_chunks(raw, chunks, context.theme)).into()
                }
                // Preview still rendering
                None => raw.into(),
            }
        } else {
            raw.into()
        }
    }
}

/// Anything that can be converted to text can be drawn
impl Draw for TemplatePreview {
    fn draw(&self, context: &mut DrawContext, _: (), chunk: Rect) {
        let text = self.to_tui(context);
        context.frame.render_widget(Paragraph::new(text), chunk);
    }
}

/// Convert chunks into a series of spans, which can be turned into a line
fn stitch_chunks<'a>(
    raw: &'a str,
    chunks: &'a [TemplateChunk],
    theme: &Theme,
) -> Vec<Span<'a>> {
    // TODO don't eat line breaks
    chunks
        .iter()
        .map(|chunk| match &chunk {
            TemplateChunk::Raw { start, end } => raw[*start..*end].into(),
            TemplateChunk::Rendered(value) => {
                Span::styled(value, theme.template_preview_text)
            }
            // There's no good way to render the entire error inline
            TemplateChunk::Error(_) => {
                Span::styled("Error", theme.template_preview_error)
            }
        })
        .collect()
}
