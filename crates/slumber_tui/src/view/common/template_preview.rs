use crate::{
    context::TuiContext,
    message::Message,
    view::{draw::Generate, util::highlight, ViewContext},
};
use ratatui::{
    buffer::Buffer,
    prelude::Rect,
    style::Style,
    text::{Line, Span, Text},
    widgets::Widget,
};
use slumber_core::{
    collection::ProfileId,
    http::content_type::ContentType,
    template::{Template, TemplateChunk},
};
use std::{
    mem,
    ops::Deref,
    sync::{Arc, Mutex},
};

/// A preview of a template string, which can show either the raw text or the
/// rendered version. The global config is used to enable/disable previews.
#[derive(Debug)]
pub struct TemplatePreview {
    /// Text to display, which could be either the raw template, or the
    /// rendered template. Either way, it may or may not be syntax
    /// highlighted. On init we send a message which will trigger a task to
    /// start the render. When the task is done, it'll call a callback to set
    /// generate the text and cache it here. This means we don't have to
    /// restitch the chunks or reapply highlighting on every render. Arc is
    /// needed to make the callback 'static.
    ///
    /// This should only ever be written to once, but we can't use `OnceLock`
    /// because it also gets an initial value. There should be effectively zero
    /// contention on the mutex because of the single write, and reads being
    /// single-threaded.
    text: Arc<Mutex<Text<'static>>>,
}

impl TemplatePreview {
    /// Create a new template preview. This will spawn a background task to
    /// render the template, *if* template preview is enabled. Profile ID
    /// defines which profile to use for the render. Optionally provide content
    /// type to enable syntax highlighting, which will be applied to both
    /// unrendered and rendered content.
    pub fn new(
        template: Template,
        profile_id: Option<ProfileId>,
        content_type: Option<ContentType>,
    ) -> Self {
        // Calculate raw text
        let text = highlight::highlight_if(
            content_type,
            // We have to clone the template to detach the lifetime. We're
            // choosing to pay one upfront cost here so we don't have to
            // recompute the text on each render. Ideally we could hold onto
            // the template and have this text reference it, but that would be
            // self-referential
            template.display().into_owned().into(),
        );
        let text = Arc::new(Mutex::new(text));

        // Trigger a task to render the preview and write the answer back into
        // the mutex
        if TuiContext::get().config.preview_templates {
            let destination = Arc::clone(&text);
            let on_complete = move |c| {
                Self::calculate_rendered_text(c, &destination, content_type)
            };

            ViewContext::send_message(Message::TemplatePreview {
                template,
                profile_id: profile_id.clone(),
                on_complete: Box::new(on_complete),
            });
        }

        Self { text }
    }

    /// Generate text from the rendered template, and replace the text in the
    /// mutex
    fn calculate_rendered_text(
        chunks: Vec<TemplateChunk>,
        destination: &Mutex<Text<'static>>,
        content_type: Option<ContentType>,
    ) {
        let text = TextStitcher::stitch_chunks(&chunks);
        let text = highlight::highlight_if(content_type, text);
        *destination
            .lock()
            .expect("Template preview text lock is poisoned") = text;
    }

    pub fn text(&self) -> impl '_ + Deref<Target = Text<'static>> {
        self.text
            .lock()
            .expect("Template preview text lock is poisoned")
    }
}

/// Clone internal text. Only call this for small pieces of text
impl Generate for &TemplatePreview {
    type Output<'this> =  Text<'this>
    where
        Self: 'this;

    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this,
    {
        self.text().clone()
    }
}

impl Widget for &TemplatePreview {
    fn render(self, area: Rect, buf: &mut Buffer) {
        self.text().deref().render(area, buf)
    }
}

/// A helper for stitching rendered template chunks into ratatui `Text`. This
/// requires some effort because ratatui *loves* line breaks, so we have to
/// very manually construct the text to make sure the structure reflects the
/// line breaks in the input.
///
/// See ratatui docs: <https://docs.rs/ratatui/latest/ratatui/text/index.html>
#[derive(Debug, Default)]
struct TextStitcher {
    completed_lines: Vec<Line<'static>>,
    next_line: Vec<Span<'static>>,
}

impl TextStitcher {
    /// Convert chunks into a series of spans, which can be turned into a line
    fn stitch_chunks(chunks: &[TemplateChunk]) -> Text<'static> {
        let styles = &TuiContext::get().styles;

        // Each chunk will get its own styling, but we can't just make each
        // chunk a Span, because one chunk might have multiple lines. And we
        // can't make each chunk a Line, because multiple chunks might be
        // together on the same line. So we need to walk down each line and
        // manually split the lines
        let mut stitcher = Self::default();
        for chunk in chunks {
            let chunk_text = Self::get_chunk_text(chunk);
            let style = match chunk {
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
    fn add_chunk(&mut self, mut chunk_text: String, style: Style) {
        // If we've reached a line ending, push the line and start a new one.
        // Intentionally ignore \r; it won't cause any harm in the output text
        match chunk_text.find('\n') {
            Some(index) => {
                // Exclude newline. +1 is safe because we know index points to
                // a char and therefore is before the end of the string
                let rest = chunk_text.split_off(index + 1);
                let popped = chunk_text.pop(); // Pop the newline
                debug_assert_eq!(popped, Some('\n'));

                self.add_span(chunk_text, style);
                self.end_line();

                // Recursion!
                // If the newline was the last char, this chunk will be empty
                if !rest.is_empty() {
                    self.add_chunk(rest, style);
                }
            }
            // This chunk has no line breaks, just add it and move on
            None => self.add_span(chunk_text, style),
        }
    }

    /// Get the renderable text for a chunk of a template. This will clone the
    /// text out of the chunk, because it's all stashed behind Arcs
    fn get_chunk_text(chunk: &TemplateChunk) -> String {
        match chunk {
            TemplateChunk::Raw(text) => text.deref().clone(),
            TemplateChunk::Rendered { value, sensitive } => {
                if *sensitive {
                    // Hide sensitive values. Ratatui has a Masked type, but
                    // it complicates the string ownership a lot and also
                    // exposes the length of the sensitive text
                    "<sensitive>".into()
                } else {
                    // We could potentially use MaybeStr to show binary data as
                    // hex, but that could get weird if there's text data in the
                    // template as well. This is simpler and prevents giant
                    // binary blobs from getting rendered in.
                    std::str::from_utf8(value).unwrap_or("<binary>").to_owned()
                }
            }
            // There's no good way to render the entire error inline
            TemplateChunk::Error(_) => "Error".into(),
        }
    }

    fn add_span(&mut self, text: String, style: Style) {
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
    fn into_text(mut self) -> Text<'static> {
        self.end_line(); // Make sure to include whatever wasn't finished
        Text::from(self.completed_lines)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{harness, TestHarness};
    use indexmap::indexmap;
    use pretty_assertions::assert_eq;
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
            collection: collection.into(),
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
