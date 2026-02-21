use crate::{
    message::Message,
    view::{
        UpdateContext, ViewContext,
        component::{Component, ComponentId},
        event::{BroadcastEvent, Emitter, Event, EventMatch, ToEmitter},
    },
};
use ratatui::{
    style::{Style, Styled},
    text::{Line, Span, Text},
};
use slumber_template::{LazyValue, RenderedChunk, RenderedOutput, Template};
use std::ops::Deref;

/// Generate template preview text
///
/// This component is a text *generator*. It does not store the text itself.
/// Different consumers do different things with the resulting text (e.g. the
/// recipe body puts it in a `TextWindow`), so it's up to the parent to decide
/// how to store and display.
///
/// This works by spawning a background task to render the template, and
/// emitting an event whenever the template text changes. An event is **not**
/// emitted for the initial text. Instead, the initial text is returned from
/// [Self::new]. Avoiding an emitted event on startup avoids some issues in
/// tests with loose emitted events.
#[derive(Debug)]
pub struct TemplatePreview {
    id: ComponentId,
    /// Template being rendered
    ///
    /// We have to hang onto this so we can re-render if there's a refresh
    /// event
    template: Template,
    /// Emitter for events whenever new text is rendered
    emitter: Emitter<TemplatePreviewEvent>,
    /// Does this component of the recipe support streaming? If so, the
    /// template will be rendered to a stream if possible and its metadata will
    /// be displayed rather than the resolved value.
    can_stream: bool,
    /// Is the text a user-given override? This changes the styling
    is_override: bool,
}

impl TemplatePreview {
    /// Create a new template preview
    ///
    /// If the template is dynamic, this will spawn a task to render the preview
    /// in the background. There will be a subsequent [TemplatePreviewEvent]
    /// emitted with the rendered text.
    ///
    ///
    /// In addition to returning the preview component, this also returns the
    /// template's input string rendered as text. This should be shown until the
    /// preview is available.
    ///
    /// ## Params
    ///
    /// - `template`: Template to be displayed/rendered
    /// - `can_stream`: Does the consumer support streaming template output? If
    ///   `true`, streams will *not* be resolved, and instead displayed as
    ///   metadata. If `false`, streams will be resolved in the preview.
    /// - `is_override`: Is the template a single-session override? For styling
    pub fn new(
        template: Template,
        can_stream: bool,
        is_override: bool,
    ) -> (Self, Text<'static>) {
        let slf = Self {
            id: ComponentId::new(),
            template,
            emitter: Emitter::default(),
            can_stream,
            is_override,
        };
        slf.render_preview(); // Render preview in the background

        // Render the initial text as well so it can be shown while the preview
        // is rendering
        let style = slf.style();
        let initial_text =
            Text::styled(slf.template.display().to_string(), style);

        (slf, initial_text)
    }

    /// Send a message to render a preview of the template in the background
    ///
    /// If preview rendering is disabled or the template is static, this will
    /// do nothing.
    fn render_preview(&self) {
        let config = &ViewContext::config();

        // If the template is static, skip the indirection
        if config.tui.preview_templates && self.template.is_dynamic() {
            let style = self.style();
            let emitter = self.emitter;
            let on_complete = move |output| {
                // Stitch the output together into Text
                let text = TextStitcher::stitch_chunks(output).set_style(style);

                // We can emit the event directly from the callback because
                // the task is run on a local set
                emitter.emit(TemplatePreviewEvent(text));
            };

            ViewContext::push_message(Message::TemplatePreview {
                template: self.template.clone(),
                can_stream: self.can_stream,
                on_complete: Box::new(on_complete),
            });
        }
    }

    fn style(&self) -> Style {
        if self.is_override {
            ViewContext::styles().text.edited
        } else {
            Style::default()
        }
    }
}

impl Component for TemplatePreview {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn update(&mut self, _: &mut UpdateContext, event: Event) -> EventMatch {
        event.m().broadcast(|event| {
            // Update text with emitted event from the preview task
            if let BroadcastEvent::RefreshPreviews = event {
                self.render_preview();
            }
        })
    }
}

impl ToEmitter<TemplatePreviewEvent> for TemplatePreview {
    fn to_emitter(&self) -> Emitter<TemplatePreviewEvent> {
        self.emitter
    }
}

/// Emitted event from [TemplatePreview] containing rendered text for a template
#[derive(Debug)]
pub struct TemplatePreviewEvent(pub Text<'static>);

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
        let styles = ViewContext::styles();

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
        if let Some(first_line) = lines.next()
            && !first_line.is_empty()
        {
            self.text
                .push_span(Span::styled(first_line.to_owned(), style));
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
    use crate::view::test_util::{TestHarness, harness};
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
        TemplatePreview::new(template, false, false);
        if should_send {
            assert_matches!(
                harness.messages_rx().try_pop(),
                Some(Message::TemplatePreview { .. })
            );
        } else {
            harness.messages_rx().assert_empty();
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

        let chunks = template.render(&context).await;
        let text = TextStitcher::stitch_chunks(chunks);
        assert_eq!(text, Text::from(expected));
    }

    /// Style some text as rendered
    fn rendered(text: &str) -> Span<'_> {
        Span::styled(text, ViewContext::styles().template_preview.text)
    }

    /// Style some text as an error
    fn error(text: &str) -> Span<'_> {
        Span::styled(text, ViewContext::styles().template_preview.error)
    }
}
