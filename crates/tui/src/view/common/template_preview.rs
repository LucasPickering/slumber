use crate::{
    message::Message,
    view::{
        UpdateContext, ViewContext,
        component::{Component, ComponentId},
        event::{BroadcastEvent, Emitter, Event, EventMatch, ToEmitter},
        persistent::{PersistentStore, SessionKey},
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
/// emitted for the initial text. Instead, call [Self::render_raw] to get the
/// initial raw template text. Avoiding an emitted event on startup avoids some
/// issues in tests with loose emitted events.
///
/// In addition to handling the preview, this also handles template overriding.
/// Use [Self::set_override] and [Self::reset_override] to modify the override.
///
/// `PK` is the persistent key used to store override state in the session store
#[derive(Debug)]
pub struct TemplatePreview<PK> {
    id: ComponentId,
    /// The template from the collection
    original_template: Template,
    /// Temporary override entered by the user
    override_template: Option<Template>,
    /// Session store key to persist the override template
    persistent_key: PK,
    /// Emitter for events whenever new text is rendered
    emitter: Emitter<TemplatePreviewEvent>,
    /// Does this component of the recipe support streaming? If so, the
    /// template will be rendered to a stream if possible and its metadata will
    /// be displayed rather than the resolved value.
    can_stream: bool,
}

impl<PK> TemplatePreview<PK> {
    /// Create a new template preview
    ///
    /// If the template is dynamic, this will spawn a task to render the preview
    /// in the background. There will be a subsequent [TemplatePreviewEvent]
    /// emitted with the rendered text.
    ///
    /// ## Params
    ///
    /// - `persistent_key`: Key under which to persist the override template in
    ///   the session store
    /// - `template`: Template to be displayed/rendered
    /// - `can_stream`: Does the consumer support streaming template output? If
    ///   `true`, streams will *not* be resolved, and instead displayed as
    ///   metadata. If `false`, streams will be resolved in the preview.
    pub fn new(persistent_key: PK, template: Template, can_stream: bool) -> Self
    where
        PK: SessionKey<Value = Template>,
    {
        let override_template = PersistentStore::get_session(&persistent_key);
        let slf = Self {
            id: ComponentId::new(),
            original_template: template,
            override_template,
            persistent_key,
            emitter: Emitter::default(),
            can_stream,
        };
        slf.render_preview(); // Render preview in the background
        slf
    }

    /// Get the active template. If an override is present, return that.
    /// Otherwise return the original.
    pub fn template(&self) -> &Template {
        self.override_template
            .as_ref()
            .unwrap_or(&self.original_template)
    }

    /// Override the recipe with a new template
    pub fn set_override(&mut self, template: Template) {
        if template == self.original_template {
            // If this matches the original template, it's not an override
            self.set_override_opt(None);
        } else if Some(&template) != self.override_template.as_ref() {
            // Only rerender if the override changed
            self.set_override_opt(Some(template));
        }
    }

    /// Reset the template override to the default from the recipe, and
    /// recompute the template preview
    pub fn reset_override(&mut self) {
        self.set_override_opt(None);
    }

    /// Internal helper to set/reset the override template and refresh the
    /// preview
    fn set_override_opt(&mut self, override_template: Option<Template>) {
        self.override_template = override_template;

        // The template has changed, so we should show the raw template while
        // the preview is rendering
        let raw_text = self.render_raw();
        self.emitter.emit(TemplatePreviewEvent(raw_text));

        self.render_preview();
    }

    /// Is a override template set?
    pub fn is_overridden(&self) -> bool {
        self.override_template.is_some()
    }

    /// Convert the raw template (without any preview rendering) into `Text` for
    /// display
    pub fn render_raw(&self) -> Text<'static> {
        Text::styled(self.template().display().to_string(), self.style())
    }

    /// Send a message to render a preview of the template in the background
    ///
    /// If preview rendering is disabled or the template is static, this will
    /// do nothing.
    fn render_preview(&self) {
        // If preview is disabled or the template is static, can skip the work
        let config = &ViewContext::config();

        if config.tui.preview_templates && self.template().is_dynamic() {
            let emitter = self.emitter;
            let style = self.style();
            let on_complete = move |output| {
                // Stitch the output together into Text
                let text = TextStitcher::stitch_chunks(output).set_style(style);

                // We can emit the event directly from the callback because
                // the task is run on a local set
                emitter.emit(TemplatePreviewEvent(text));
            };

            ViewContext::send_message(Message::TemplatePreview {
                template: self.template().clone(),
                can_stream: self.can_stream,
                on_complete: Box::new(on_complete),
            });
        }
    }

    fn style(&self) -> Style {
        if self.override_template.is_some() {
            ViewContext::styles().text.edited
        } else {
            Style::default()
        }
    }
}

impl<PK> Component for TemplatePreview<PK>
where
    PK: Clone + SessionKey<Value = Template>,
{
    fn id(&self) -> ComponentId {
        self.id
    }

    fn persist(&self, store: &mut PersistentStore) {
        // Persist to the session store. Overrides are meant to be temporary, so
        // we don't want to encourage users to rely on them long-term. They
        // should be making edits to their YAML file instead.
        if let Some(template) = &self.override_template {
            store.set_session(self.persistent_key.clone(), template.clone());
        } else {
            store.remove_session(&self.persistent_key);
        }
    }

    fn update(&mut self, _: &mut UpdateContext, event: Event) -> EventMatch {
        event
            .m()
            // Update text with emitted event from the preview task
            .broadcast(|event| {
                if let BroadcastEvent::RefreshPreviews = event {
                    self.render_preview();
                }
            })
    }
}

impl<PK> ToEmitter<TemplatePreviewEvent> for TemplatePreview<PK> {
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

    #[derive(Debug, PartialEq)]
    struct TestKey;

    impl SessionKey for TestKey {
        type Value = Template;
    }

    /// TemplatePreview message should only be sent for dynamic templates
    #[rstest]
    #[case::static_("static!", false)]
    #[case::dynamic("{{ dynamic }}", true)]
    fn test_send_message(
        mut harness: TestHarness,
        #[case] template: Template,
        #[case] should_send: bool,
    ) {
        TemplatePreview::new(TestKey, template, false);
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
        Span::styled(text, ViewContext::styles().template_preview.text)
    }

    /// Style some text as an error
    fn error(text: &str) -> Span<'_> {
        Span::styled(text, ViewContext::styles().template_preview.error)
    }
}
