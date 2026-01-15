use crate::{
    context::TuiContext,
    message::Message,
    view::{
        UpdateContext, ViewContext,
        component::{Canvas, Component, ComponentId, Draw, DrawMetadata},
        event::{BroadcastEvent, Emitter, Event, EventMatch},
        state::Identified,
        util::highlight,
    },
};
use ratatui::{
    style::{Style, Styled},
    text::{Line, Span, Text},
};
use slumber_core::http::content_type::ContentType;
use slumber_template::{LazyValue, RenderedChunk, RenderedOutput, Template};
use std::ops::Deref;

/// A preview of a template string, which can show either the raw text or the
/// rendered version. The global config is used to enable/disable previews.
#[derive(Debug)]
pub struct TemplatePreview {
    id: ComponentId,
    /// Emitter for rendered text from the preview task
    callback_emitter: Emitter<Identified<Text<'static>>>,
    template: Template,
    /// Content-Type of the output, which can be used to apply syntax
    /// highlighting
    content_type: Option<ContentType>,
    /// Has the template been overridden by the user in the current session?
    /// Applies additional styling
    overridden: bool,
    /// Does this component of the recipe support streaming? If so, the
    /// template will be rendered to a stream if possible and its metadata will
    /// be displayed rather than the resolved value.
    can_stream: bool,
    /// Text to display, which could be either the raw template, or the
    /// rendered template. Either way, it may or may not be syntax
    /// highlighted. On init we send a message which will trigger a task to
    /// start the render. When the task is done, it'll emit an event to set the
    /// text.
    text: Identified<Text<'static>>,
}

impl TemplatePreview {
    /// Create a new template preview. This will spawn a background task to
    /// render the template, *if* template preview is enabled. Profile ID
    /// defines which profile to use for the render. Optionally provide content
    /// type to enable syntax highlighting, which will be applied to both
    /// unrendered and rendered content.
    ///
    /// ## Params
    ///
    /// - `template`: Template to render
    /// - `content_type`: Content-Type of the output, which can be used to apply
    ///   syntax highlighting
    /// - `overridden`: Has the template been overridden by the user in the
    ///   current session? Applies additional styling
    /// - `can_stream`: Does this component of the recipe support streaming? If
    ///   so, the template will be rendered to a stream if possible and its
    ///   metadata will be displayed rather than the resolved value.
    pub fn new(
        template: Template,
        content_type: Option<ContentType>,
        overridden: bool,
        can_stream: bool,
    ) -> Self {
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
        .set_style(Self::style(overridden))
        .into();

        let mut slf = Self {
            id: ComponentId::new(),
            callback_emitter: Emitter::default(),
            template,
            content_type,
            overridden,
            can_stream,
            text,
        };
        slf.render_preview();
        slf
    }

    pub fn text(&self) -> &Identified<Text<'static>> {
        &self.text
    }

    /// Send a message triggering a render of this template. The rendered
    /// preview will be stored back in our lock. Used for both initial render
    /// and refreshes.
    fn render_preview(&mut self) {
        // Trigger a task to render the preview and write the answer back into
        // the mutex. If the template is static (has no dynamic chunks), there's
        // no need to do this. We'll display the raw template text by default,
        // which will be equivalent to the rendered text
        let config = &TuiContext::get().config;
        if config.tui.preview_templates && self.template.is_dynamic() {
            let content_type = self.content_type;
            let style = Self::style(self.overridden);
            let emitter = self.callback_emitter;
            let on_complete = move |output| {
                // Stitch the output together into Text, then apply highlighting
                let text = TextStitcher::stitch_chunks(output);
                let text = highlight::highlight_if(content_type, text)
                    .set_style(style);
                // We can emit the event directly from the callback because
                // the task is run on a local set. This is maybe a bit jank and
                // it should be routed through Message instead?
                emitter.emit(text.into());
            };

            ViewContext::send_message(Message::TemplatePreview {
                template: self.template.clone(),
                can_stream: self.can_stream,
                on_complete: Box::new(on_complete),
            });
        }
    }

    /// Get styling for the preview, based on overridden state
    fn style(overridden: bool) -> Style {
        if overridden {
            TuiContext::get().styles.text.edited
        } else {
            Style::default()
        }
    }
}

impl From<Template> for TemplatePreview {
    fn from(template: Template) -> Self {
        Self::new(template, None, false, false)
    }
}

impl Component for TemplatePreview {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn update(&mut self, _: &mut UpdateContext, event: Event) -> EventMatch {
        event
            .m()
            // Update text with emitted event from the preview task
            .emitted(self.callback_emitter, |text| self.text = text)
            .broadcast(|event| {
                if let BroadcastEvent::RefreshPreviews = event {
                    self.render_preview();
                }
            })
    }
}

impl Draw for TemplatePreview {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        canvas.render_widget(&**self.text(), metadata.area());
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
    fn stitch_chunks(chunks: RenderedOutput) -> Text<'static> {
        let styles = &TuiContext::get().styles;

        // Each chunk will get its own styling, but we can't just make each
        // chunk a Span, because one chunk might have multiple lines. And we
        // can't make each chunk a Line, because multiple chunks might be
        // together on the same line. So we need to walk down each line and
        // manually split the lines
        let mut stitcher = Self::default();
        for chunk in chunks {
            let style = match chunk {
                RenderedChunk::Raw(_) => Style::default(),
                RenderedChunk::Rendered { .. } => styles.template_preview.text,
                RenderedChunk::Error(_) => styles.template_preview.error,
            };
            let chunk_text = Self::get_chunk_text(chunk);

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
    fn get_chunk_text(chunk: RenderedChunk) -> String {
        match chunk {
            RenderedChunk::Raw(text) => text.deref().into(),
            RenderedChunk::Rendered(lazy) => match lazy {
                LazyValue::Value(value) => {
                    // We could potentially use MaybeStr to show binary data as
                    // hex, but that could get weird if there's text data in the
                    // template as well. This is simpler and prevents giant
                    // binary blobs from getting rendered in.
                    value
                        .try_into_string()
                        .unwrap_or_else(|_| "<binary>".into())
                }
                LazyValue::Stream { source, .. } => {
                    format!("<{source}>")
                }
                // Stringify all the nested chunks and concat them together.
                // Nested chunks can be generated by a profile field. This whole
                // thing will get styled as dynamic, even if it contains raw
                // chunks within.
                LazyValue::Nested(output) => {
                    output.into_iter().map(Self::get_chunk_text).collect()
                }
            },
            // There's no good way to render the entire error inline
            RenderedChunk::Error(_) => "Error".into(),
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
        collection::{Collection, Profile},
        render::TemplateContext,
        test_util::by_id,
    };
    use slumber_util::{Factory, assert_matches};

    /// TemplatePreview message should only be sent for dynamic templates
    #[rstest]
    #[case::static_("static!", false)]
    #[case::dynamic("{{ dynamic }}", true)]
    fn test_send_message(
        mut harness: TestHarness,
        #[case] template: Template,
        #[case] should_send: bool,
    ) {
        TemplatePreview::new(template, None, false, false);
        if should_send {
            assert_matches!(
                harness.messages().pop_now(),
                Message::TemplatePreview { .. }
            );
        } else {
            harness.messages().assert_empty();
        }
    }

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
        r"binary data: {{ b'\xc3\x28' }}",
        vec![Line::from(vec![Span::raw("binary data: "), rendered("<binary>")])]
    )]
    #[tokio::test]
    async fn test_template_stitch(
        _harness: TestHarness,
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
        let collection = Collection {
            profiles: by_id([profile]),
            ..Collection::factory(())
        };
        let context = TemplateContext {
            collection: collection.into(),
            selected_profile: Some(profile_id),
            ..TemplateContext::factory(())
        };

        let chunks = template.render(&context.streaming(false)).await;
        let text = TextStitcher::stitch_chunks(chunks);
        assert_eq!(text, Text::from(expected));
    }

    /// Style some text as rendered
    fn rendered(text: &str) -> Span<'_> {
        Span::styled(text, TuiContext::get().styles.template_preview.text)
    }

    /// Style some text as an error
    fn error(text: &str) -> Span<'_> {
        Span::styled(text, TuiContext::get().styles.template_preview.error)
    }
}
