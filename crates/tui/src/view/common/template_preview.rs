use crate::{
    context::TuiContext,
    message::Message,
    view::{ViewContext, draw::Generate, state::Identified, util::highlight},
};
use ratatui::{
    buffer::Buffer,
    prelude::Rect,
    style::{Style, Styled},
    text::{Line, Span, Text},
    widgets::Widget,
};
use slumber_core::{
    http::content_type::ContentType,
    template::{Template, TemplateChunk},
};
use std::{
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
    text: Arc<Mutex<Identified<Text<'static>>>>,
}

impl TemplatePreview {
    /// Create a new template preview. This will spawn a background task to
    /// render the template, *if* template preview is enabled. Profile ID
    /// defines which profile to use for the render. Optionally provide content
    /// type to enable syntax highlighting, which will be applied to both
    /// unrendered and rendered content.
    pub fn new(
        template: Template,
        content_type: Option<ContentType>,
        overridden: bool,
    ) -> Self {
        let tui_context = TuiContext::get();
        let style = if overridden {
            tui_context.styles.text.edited
        } else {
            Style::default()
        };

        // Calculate raw text
        let text: Identified<Text> = highlight::highlight_if(
            content_type,
            // We have to clone the template to detach the lifetime. We're
            // choosing to pay one upfront cost here so we don't have to
            // recompute the text on each render. Ideally we could hold onto
            // the template and have this text reference it, but that would be
            // self-referential
            template.display().into_owned().into(),
        )
        .set_style(style)
        .into();
        let text = Arc::new(Mutex::new(text));

        // Trigger a task to render the preview and write the answer back into
        // the mutex
        if tui_context.config.preview_templates {
            let destination = Arc::clone(&text);
            let on_complete = move |c| {
                Self::calculate_rendered_text(
                    c,
                    &destination,
                    content_type,
                    style,
                );
            };

            ViewContext::send_message(Message::TemplatePreview {
                template,
                on_complete: Box::new(on_complete),
            });
        }

        Self { text }
    }

    pub fn text(&self) -> impl '_ + Deref<Target = Identified<Text<'static>>> {
        self.text
            .lock()
            .expect("Template preview text lock is poisoned")
    }

    /// Generate text from the rendered template, and replace the text in the
    /// mutex
    fn calculate_rendered_text(
        chunks: Vec<TemplateChunk>,
        destination: &Mutex<Identified<Text<'static>>>,
        content_type: Option<ContentType>,
        style: Style,
    ) {
        let text = TextStitcher::stitch_chunks(&chunks);
        let text = highlight::highlight_if(content_type, text).set_style(style);
        *destination
            .lock()
            .expect("Template preview text lock is poisoned") = text.into();
    }
}

impl From<Template> for TemplatePreview {
    fn from(template: Template) -> Self {
        Self::new(template, None, false)
    }
}

/// Clone internal text. Only call this for small pieces of text
impl Generate for &TemplatePreview {
    type Output<'this>
        = Text<'this>
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
        (&**self.text()).render(area, buf);
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
    text: Text<'static>,
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
        stitcher.text
    }

    /// Add one chunk to the text. This will recursively split on any line
    /// breaks in the text until it reaches the end.
    fn add_chunk(&mut self, chunk_text: String, style: Style) {
        let ends_in_newline = chunk_text.ends_with('\n');

        // The first line should extend the final line of the current text,
        // because there isn't necessarily a line break between chunks
        let mut lines = chunk_text.lines();
        if let Some(first_line) = lines.next() {
            if !first_line.is_empty() {
                self.text
                    .push_span(Span::styled(first_line.to_owned(), style));
            }
        }
        self.text.extend(lines.map(|line| {
            // If the text is empty, push an empty line instead of a line with
            // a single empty chunk
            if line.is_empty() {
                Line::default()
            } else {
                // Push a span instead of a whole line, because if this is the
                // last line, the next chunk may extend it
                Span::styled(line.to_owned(), style).into()
            }
        }));

        // std::lines throws away trailing newlines, but we care about them
        // because the next chunk needs to go on a new line. We also care about
        // keeping trailing newlines at the end of HTTP bodies, for correctness
        if ends_in_newline {
            self.text.push_line(Line::default());
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{TestHarness, harness};
    use indexmap::indexmap;
    use pretty_assertions::assert_eq;
    use rstest::rstest;
    use slumber_core::{
        collection::{Chain, ChainSource, Collection, Profile},
        template::TemplateContext,
        test_util::{by_id, invalid_utf8_chain},
    };
    use slumber_util::Factory;

    /// Test line breaks, multi-byte characters, and binary data
    #[rstest]
    #[case::line_breaks(
        // Test these cases related to line breaks:
        // - Line break within a raw chunk
        // - Chunk is just a line break
        // - Line break within a rendered chunk
        // - Line break at chunk boundary
        // - NO line break at chunk boundary
        // - Consecutive line breaks
        "intro\n{{simple}}\n{{emoji}} 💚💙💜 {{unknown}}\n\noutro\r\nmore outro\n",
        vec![
            Line::from("intro"),
            Line::from(rendered("ww")),
            Line::from(rendered("🧡")),
            Line::from(vec![
                rendered("💛"),
                Span::raw(" 💚💙💜 "),
                error("Error"),
            ]),
            Line::from(""),
            Line::from("outro"),
            Line::from("more outro"),
            Line::from(""), // Trailing newline
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
        let profile_data = indexmap! {
            "simple".into() => "ww".into(),
            "emoji".into() => "🧡\n💛".into()
        };
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
