use crate::{
    context::TuiContext,
    message::Message,
    view::{draw::Generate, ViewContext},
};
use ratatui::{
    buffer::Buffer,
    prelude::Rect,
    style::Style,
    text::{Line, Span, Text},
    widgets::{Paragraph, Widget},
};
use slumber_core::{
    collection::ProfileId,
    template::{Template, TemplateChunk},
};
use std::{
    mem,
    sync::{Arc, OnceLock},
};

/// A preview of a template string, which can show either the raw text or the
/// rendered version. This switch is stored in render context, so it can be
/// changed globally.
#[derive(Debug)]
pub enum TemplatePreview {
    /// Template previewing is disabled, just show the raw text
    Disabled { template: Template },
    /// Template previewing is enabled, render the template
    Enabled {
        template: Template,
        /// Rendered chunks. On init we send a message which will trigger a
        /// task to start the render. When the task is done, it'll dump
        /// its result back here.
        chunks: Arc<OnceLock<Vec<TemplateChunk>>>,
    },
}

impl TemplatePreview {
    /// Create a new template preview. This will spawn a background task to
    /// render the template, *if* template preview is enabled. Profile ID
    /// defines which profile to use for the render.
    pub fn new(template: Template, profile_id: Option<ProfileId>) -> Self {
        if TuiContext::get().config.preview_templates {
            let chunks = Arc::new(OnceLock::new());
            ViewContext::send_message(Message::TemplatePreview {
                // If this is a bottleneck we can Arc it
                template: template.clone(),
                profile_id: profile_id.clone(),
                destination: Arc::clone(&chunks),
            });

            Self::Enabled { template, chunks }
        } else {
            Self::Disabled { template }
        }
    }
}

impl Generate for &TemplatePreview {
    type Output<'this> = Text<'this>
    where
        Self: 'this;

    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this,
    {
        match self {
            TemplatePreview::Disabled { template } => template.display().into(),
            // If the preview render is ready, show it. Otherwise fall back
            // to the raw
            TemplatePreview::Enabled {
                template, chunks, ..
            } => match chunks.get() {
                Some(chunks) => TextStitcher::stitch_chunks(chunks),
                // Preview still rendering
                None => template.display().into(),
            },
        }
    }
}

impl Widget for &TemplatePreview {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let text = self.generate();
        Paragraph::new(text).render(area, buf)
    }
}

/// A helper for stitching rendered template chunks into ratatui `Text`. This
/// requires some effort because ratatui *loves* line breaks, so we have to
/// very manually construct the text to make sure the structure reflects the
/// line breaks in the input.
///
/// See ratatui docs: <https://docs.rs/ratatui/latest/ratatui/text/index.html>
#[derive(Debug, Default)]
struct TextStitcher<'a> {
    completed_lines: Vec<Line<'a>>,
    next_line: Vec<Span<'a>>,
}

impl<'a> TextStitcher<'a> {
    /// Convert chunks into a series of spans, which can be turned into a line
    fn stitch_chunks(chunks: &'a [TemplateChunk]) -> Text<'a> {
        let styles = &TuiContext::get().styles;

        // Each chunk will get its own styling, but we can't just make each
        // chunk a Span, because one chunk might have multiple lines. And we
        // can't make each chunk a Line, because multiple chunks might be
        // together on the same line. So we need to walk down each line and
        // manually split the lines
        let mut stitcher = Self::default();
        for chunk in chunks {
            let chunk_text = Self::get_chunk_text(chunk);
            let style = match &chunk {
                TemplateChunk::Raw(_) => Style::default(),
                TemplateChunk::Rendered { .. } => styles.template_preview.text,
                TemplateChunk::Error(_) => styles.template_preview.error,
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
                // Recursion!
                self.add_chunk(b, style);
            }
            // This chunk has no line breaks, just add it and move on
            None => self.add_span(chunk_text, style),
        }
    }

    /// Get the renderable text for a chunk of a template
    fn get_chunk_text(chunk: &'a TemplateChunk) -> &'a str {
        match chunk {
            TemplateChunk::Raw(text) => text,
            TemplateChunk::Rendered { value, sensitive } => {
                if *sensitive {
                    // Hide sensitive values. Ratatui has a Masked type, but
                    // it complicates the string ownership a lot and also
                    // exposes the length of the sensitive text
                    "<sensitive>"
                } else {
                    // We could potentially use MaybeStr to show binary data as
                    // hex, but that could get weird if there's text data in the
                    // template as well. This is simpler and prevents giant
                    // binary blobs from getting rendered in.
                    std::str::from_utf8(value).unwrap_or("<binary>")
                }
            }
            // There's no good way to render the entire error inline
            TemplateChunk::Error(_) => "Error",
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
    use crate::test_util::{harness, TestHarness};
    use indexmap::indexmap;
    use rstest::rstest;
    use slumber_core::{
        collection::{Chain, ChainSource, Collection, Profile},
        template::TemplateContext,
        test_util::{by_id, invalid_utf8_chain, Factory},
    };

    /// Test line breaks, multi-byte characters, and binary data
    #[rstest]
    #[case::line_breaks(
        // Test these cases related to line breaks:
        // - Line break within a raw chunk
        // - Line break within a rendered chunk
        // - Line break at chunk boundary
        // - NO line break at chunk boundary
        "intro\n{{user_id}} 💚💙💜 {{unknown}}\noutro\r\nmore outro",
        vec![
            Line::from("intro"),
            Line::from(rendered("🧡")),
            Line::from(vec![
                rendered("💛"),
                Span::raw(" 💚💙💜 "),
                error("Error"),
            ]),
            Line::from("outro\r"), // \r shouldn't create any issues
            Line::from("more outro"),
        ]
    )]
    #[case::binary(
        "binary data: {{chains.binary}}",
        vec![Line::from(vec![Span::raw("binary data: "), rendered("<binary>")])]
    )]
    #[tokio::test]
    async fn test_template_stitch(
        _harness: TestHarness,
        invalid_utf8_chain: ChainSource,
        #[case] template: Template,
        #[case] expected: Vec<Line<'static>>,
    ) {
        let profile_data = indexmap! { "user_id".into() => "🧡\n💛".into() };
        let profile = Profile {
            data: profile_data,
            ..Profile::factory(())
        };
        let profile_id = profile.id.clone();
        let chain = Chain {
            id: "binary".into(),
            source: invalid_utf8_chain,
            ..Chain::factory(())
        };
        let collection = Collection {
            profiles: by_id([profile]),
            chains: by_id([chain]),
            ..Collection::factory(())
        };
        let context = TemplateContext {
            collection,
            selected_profile: Some(profile_id),
            ..TemplateContext::factory(())
        };

        let chunks = template.render_chunks(&context).await;
        let text = TextStitcher::stitch_chunks(&chunks);
        assert_eq!(text, Text::from(expected));
    }

    /// Style some text as rendered
    fn rendered(text: &str) -> Span {
        Span::styled(text, TuiContext::get().styles.template_preview.text)
    }

    /// Style some text as an error
    fn error(text: &str) -> Span {
        Span::styled(text, TuiContext::get().styles.template_preview.error)
    }
}
