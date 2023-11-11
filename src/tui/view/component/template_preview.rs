use crate::{
    collection::ProfileId,
    template::{Template, TemplateChunk},
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
    style::Style,
    text::{Line, Span, Text},
    widgets::Paragraph,
};
use std::{
    mem,
    ops::Deref,
    sync::{Arc, OnceLock},
};

/// A preview of a template string, which can show either the raw text or the
/// rendered version. This switch is stored in render context, so it can be
/// changed globally.
#[derive(Debug)]
pub struct TemplatePreview {
    template: Template,
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
        template: Template,
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

        if context.config.preview_templates {
            // If the preview render is ready, show it. Otherwise fall back to
            // the raw
            match self.chunks.get() {
                Some(chunks) => TextStitcher::stitch_chunks(
                    &self.template,
                    chunks,
                    context.theme,
                ),
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

/// A helper for stitching rendered template chunks into ratatui `Text`. This
/// requires some effort because ratatui *loves* line breaks, so we have to
/// very manually construct the text to make sure the structure reflects the
/// line breaks in the input.
///
/// See ratatui docs: https://docs.rs/ratatui/latest/ratatui/text/index.html
#[derive(Debug, Default)]
struct TextStitcher<'a> {
    completed_lines: Vec<Line<'a>>,
    next_line: Vec<Span<'a>>,
}

impl<'a> TextStitcher<'a> {
    /// Convert chunks into a series of spans, which can be turned into a line
    fn stitch_chunks(
        template: &'a Template,
        chunks: &'a [TemplateChunk],
        theme: &Theme,
    ) -> Text<'a> {
        // Each chunk will get its own styling, but we can't just make each
        // chunk a Span, because one chunk might have multiple lines. And we
        // can't make each chunk a Line, because multiple chunks might be
        // together on the same line. So we need to walk down each line and
        // manually split the lines
        let mut stitcher = Self::default();
        for chunk in chunks {
            let (chunk_text, style) = match &chunk {
                TemplateChunk::Raw(span) => {
                    (template.substring(*span), Style::default())
                }
                TemplateChunk::Rendered(value) => {
                    (value.as_str(), theme.template_preview_text)
                }
                // There's no good way to render the entire error inline
                TemplateChunk::Error(_) => {
                    ("Error", theme.template_preview_error)
                }
            };

            stitcher.add_chunk(chunk_text, style);
        }
        stitcher.into_text()
    }

    /// Add one chunk to the text. This will recursively split on any line
    /// breaks in the text until it reaches the end.
    fn add_chunk(&mut self, chunk_text: &'a str, style: Style) {
        // If we've reached a line ending, push the line and start a new one.
        // Intentionally ignore \r; it won't cause any harm in the output text
        match chunk_text.split_once('\n') {
            Some((a, b)) => {
                self.add_span(a, style);
                self.end_line();
                self.add_chunk(b, style);
            }
            // This chunk has no line breaks, just add it and move on
            None => self.add_span(chunk_text, style),
        }
    }

    fn add_span(&mut self, text: &'a str, style: Style) {
        if !text.is_empty() {
            self.next_line.push(Span::styled(text, style));
        }
    }

    /// Add the current line to the accumulator, and start a new one
    fn end_line(&mut self) {
        if !self.next_line.is_empty() {
            self.completed_lines
                .push(mem::take(&mut self.next_line).into());
        }
    }

    /// Convert all lines into a text block
    fn into_text(mut self) -> Text<'a> {
        self.end_line(); // Make sure to include whatever wasn't finished
        Text::from(self.completed_lines)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::factory::*;
    use factori::create;
    use indexmap::indexmap;

    /// Test these cases related to line breaks:
    /// - Line break within a raw chunk
    /// - Line break within a rendered chunk
    /// - Line break at chunk boundary
    /// - NO line break at chunk boundary
    /// Ratatui is fucky with how it handles line breaks in text, so we need
    /// to make sure our output reflects the input
    ///
    /// Additionally, test multi-byte unicode characters to make sure string
    /// offset indexes work correctly
    #[tokio::test]
    async fn test_template_stitch() {
        // Render a template
        let template = Template::parse(
            "intro\n{{user_id}} 💚💙💜 {{unknown}}\noutro\r\nmore outro".into(),
        )
        .unwrap();
        let profile = indexmap! { "user_id".into() => "🧡\n💛".into() };
        let context = create!(TemplateContext, profile: profile);
        let chunks = template.render_chunks(&context).await;
        let theme = Theme::default();

        let text = TextStitcher::stitch_chunks(&template, &chunks, &theme);
        let rendered_style = theme.template_preview_text;
        let error_style = theme.template_preview_error;
        let expected = Text::from(vec![
            Line::from("intro"),
            Line::from(Span::styled("🧡", rendered_style)),
            Line::from(vec![
                Span::styled("💛", rendered_style),
                Span::raw(" 💚💙💜 "),
                Span::styled("Error", error_style),
            ]),
            Line::from("outro\r"), // \r shouldn't create any issues
            Line::from("more outro"),
        ]);
        assert_eq!(text, expected);
    }
}
