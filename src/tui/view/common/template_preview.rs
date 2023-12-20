use crate::{
    collection::ProfileId,
    template::{Template, TemplateChunk},
    tui::{context::TuiContext, message::Message, view::draw::Generate},
};
use derive_more::Deref;
use ratatui::{
    buffer::Buffer,
    prelude::Rect,
    style::Style,
    text::{Line, Span, Text},
    widgets::{Paragraph, Widget},
};
use std::{
    fmt::{self, Display, Formatter},
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
    /// render the template. Profile ID defines which profile to use for the
    /// render.
    pub fn new(
        template: Template,
        profile_id: Option<ProfileId>,
        enabled: bool,
    ) -> Self {
        if enabled {
            // Tell the controller to start rendering the preview, and it'll
            // store it back here when done
            let lock = Arc::new(OnceLock::new());
            TuiContext::send_message(Message::TemplatePreview {
                // If this is a bottleneck we can Arc it
                template: template.clone(),
                profile_id,
                destination: Arc::clone(&lock),
            });

            Self::Enabled {
                template,
                chunks: lock,
            }
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
        // The raw template string
        match self {
            TemplatePreview::Disabled { template } => template.deref().into(),
            // If the preview render is ready, show it. Otherwise fall back
            // to the raw
            TemplatePreview::Enabled { template, chunks } => match chunks.get()
            {
                Some(chunks) => TextStitcher::stitch_chunks(template, chunks),
                // Preview still rendering
                None => template.deref().into(),
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

/// Convert to raw text. Useful for copypasta
impl Display for TemplatePreview {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            TemplatePreview::Disabled { template } => write!(f, "{template}"),
            // If the preview render is ready, show it. Otherwise fall back
            // to the raw
            TemplatePreview::Enabled { template, chunks } => match chunks.get()
            {
                Some(chunks) => {
                    for chunk in chunks {
                        write!(f, "{}", get_chunk_text(template, chunk))?;
                    }
                    Ok(())
                }
                // Preview still rendering
                None => write!(f, "{template}"),
            },
        }
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
    ) -> Text<'a> {
        let theme = &TuiContext::get().theme;

        // Each chunk will get its own styling, but we can't just make each
        // chunk a Span, because one chunk might have multiple lines. And we
        // can't make each chunk a Line, because multiple chunks might be
        // together on the same line. So we need to walk down each line and
        // manually split the lines
        let mut stitcher = Self::default();
        for chunk in chunks {
            let chunk_text = get_chunk_text(template, chunk);
            let style = match &chunk {
                TemplateChunk::Raw(_) => Style::default(),
                TemplateChunk::Rendered { .. } => theme.template_preview_text,
                TemplateChunk::Error(_) => theme.template_preview_error,
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

/// Get the plain text for a chunk of a template
fn get_chunk_text<'a>(
    template: &'a Template,
    chunk: &'a TemplateChunk,
) -> &'a str {
    match chunk {
        TemplateChunk::Raw(span) => template.substring(*span),
        TemplateChunk::Rendered { value, sensitive } => {
            if *sensitive {
                // Hide sensitive values. Ratatui has a Masked type, but
                // it complicates the string ownership a lot and also
                // exposes the length of the sensitive text
                "<sensitive>"
            } else {
                value.as_str()
            }
        }
        // There's no good way to render the entire error inline
        TemplateChunk::Error(_) => "Error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{factory::*, tui::context::tui_context};
    use factori::create;
    use indexmap::indexmap;
    use rstest::rstest;

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
    #[rstest]
    #[tokio::test]
    async fn test_template_stitch(_tui_context: ()) {
        // Render a template
        let template = Template::parse(
            "intro\n{{user_id}} 💚💙💜 {{unknown}}\noutro\r\nmore outro".into(),
        )
        .unwrap();
        let profile_data = indexmap! { "user_id".into() => "🧡\n💛".into() };
        let profile = create!(Profile, data: profile_data);
        let context = create!(TemplateContext, profile: Some(profile));
        let chunks = template.render_chunks(&context).await;
        let theme = &TuiContext::get().theme;

        let text = TextStitcher::stitch_chunks(&template, &chunks);
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
