use crate::{
    context::TuiContext,
    message::Message,
    view::{draw::Generate, state::Identified, util::highlight, ViewContext},
};
use ratatui::{
    buffer::Buffer,
    prelude::Rect,
    text::{Span, Text},
    widgets::Widget,
};
use slumber_core::{
    http::content_type::ContentType,
    template::{Template, TemplateError},
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
    pub fn new(template: Template, content_type: Option<ContentType>) -> Self {
        // Calculate raw text
        let text: Identified<Text> = highlight::highlight_if(
            content_type,
            // We have to clone the template to detach the lifetime. We're
            // choosing to pay one upfront cost here so we don't have to
            // recompute the text on each render. Ideally we could hold onto
            // the template and have this text reference it, but that would be
            // self-referential
            template.to_string().into(),
        )
        .into();
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
                on_complete: Box::new(on_complete),
            });
        }

        Self { text }
    }

    /// Generate text from the rendered template, and replace the text in the
    /// mutex
    fn calculate_rendered_text(
        result: Result<Vec<u8>, TemplateError>,
        destination: &Mutex<Identified<Text<'static>>>,
        content_type: Option<ContentType>,
    ) {
        let styles = &TuiContext::get().styles.template_preview;

        // TODO can we do chunk-based errors?
        // TODO hide sensitive values
        // We have to wrap everything in a span so the styling doesn't apply
        // to the entire text, beyond the end of the content
        let text = match result.map(String::from_utf8) {
            Ok(Ok(rendered)) => Span::styled(rendered, styles.text).into(),
            // Rendered succeeded but not UTF-8
            Ok(Err(_)) => Span::styled("<binary>", styles.text).into(),
            // Render failed
            Err(_) => Span::styled("Error", styles.error).into(),
        };
        let text = highlight::highlight_if(content_type, text);
        *destination
            .lock()
            .expect("Template preview text lock is poisoned") = text.into();
    }

    pub fn text(&self) -> impl '_ + Deref<Target = Identified<Text<'static>>> {
        self.text
            .lock()
            .expect("Template preview text lock is poisoned")
    }
}

impl From<Template> for TemplatePreview {
    fn from(template: Template) -> Self {
        Self::new(template, None)
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
        (&**self.text()).render(area, buf)
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
