use crate::{
    context::TuiContext,
    message::Message,
    view::{ViewContext, draw::Generate, state::Identified, util::highlight},
};
use ratatui::{buffer::Buffer, prelude::Rect, text::Text, widgets::Widget};
use slumber_core::{http::content_type::ContentType, template::Template};
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
        let tui_context = TuiContext::get();

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
        // the mutex. SKip this if the template is a static value (i.e. not a
        // function)
        if tui_context.config.preview_templates && template.is_dynamic() {
            let destination = Arc::clone(&text);
            let on_complete = move |result| {
                Self::calculate_rendered_text(
                    result,
                    &destination,
                    content_type,
                )
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
        result: Result<String, ()>,
        destination: &Mutex<Identified<Text<'static>>>,
        content_type: Option<ContentType>,
    ) {
        let styles = &TuiContext::get().styles;
        let text = match result {
            // TODO only apply this to function templates
            Ok(preview) => Text::styled(preview, styles.template_preview.text),
            Err(_) => Text::styled("Error", styles.template_preview.error),
        };
        let text = highlight::highlight_if(content_type, text);
        *destination
            .lock()
            .expect("Template preview text lock is poisoned") = text.into();
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
