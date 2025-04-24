use crate::{
    context::TuiContext,
    message::Message,
    view::{ViewContext, draw::Generate, state::Identified, util::highlight},
};
use petitscript::Value;
use ratatui::{
    buffer::Buffer,
    prelude::Rect,
    text::{Span, Text},
    widgets::Widget,
};
use slumber_core::{http::content_type::ContentType, render::Procedure};
use std::sync::{Arc, LazyLock, OnceLock};

/// A preview of a procedure. Initially this will show the stringified value
/// of the procedusre. On creation it will kick off a task to render the
/// procedure in preview mode, then show the result when it's ready.
#[derive(Debug)]
pub struct Preview {
    text: PreviewText,
}

impl Preview {
    /// Create a new procedure preview. This will spawn a background task to
    /// render the procedure. Profile ID defines which profile to use for the
    /// render. Optionally provide content type to enable syntax highlighting,
    /// which will be applied to both unrendered and rendered content.
    pub fn new(
        procedure: Procedure,
        content_type: Option<ContentType>,
    ) -> Self {
        let text = if procedure.is_dynamic() {
            // Procedure is dynamic - kick off a render. We'll show a
            // placeholder until then. We *could* show a stringification of the
            // procedure, but it's just going to be a placeholder from PS
            let text = Arc::new(OnceLock::new());
            let destination = Arc::clone(&text);
            let on_complete = move |result| {
                Self::calculate_rendered_text(
                    result,
                    &destination,
                    content_type,
                )
            };

            ViewContext::send_message(Message::Preview {
                procedure,
                on_complete: Box::new(on_complete),
            });

            PreviewText::Dynamic(text)
        } else {
            // We have a static value, just stringify it
            PreviewText::Static(
                highlight::highlight_if(
                    content_type,
                    Self::value_to_text(procedure.into_value(), content_type)
                        .into(),
                )
                .into(),
            )
        };

        Self { text }
    }

    /// Get visible preview text
    pub fn text(&self) -> &Identified<Text<'static>> {
        static PLACEHOLDER: LazyLock<Identified<Text<'static>>> =
            LazyLock::new(|| Identified::new("Rendering...".into()));

        match &self.text {
            PreviewText::Static(text) => text,
            PreviewText::Dynamic(lock) => {
                lock.get().unwrap_or_else(|| &*PLACEHOLDER)
            }
        }
    }

    /// Generate text from the rendered template, and replace the text in the
    /// mutex
    fn calculate_rendered_text(
        result: Result<Value, ()>,
        destination: &OnceLock<Identified<Text<'static>>>,
        content_type: Option<ContentType>,
    ) {
        let styles = &TuiContext::get().styles;
        let text = match result {
            Ok(value) => {
                // Convert the value to a string according to its content type
                let text = Self::value_to_text(value, content_type);
                Text::styled(text, styles.preview.text)
            }
            Err(_) => Span::styled("Error", styles.preview.error).into(),
        };
        let text = highlight::highlight_if(content_type, text);
        // SAFETY: This is the only place that sets the preview and we only kick
        // this off once
        destination
            .set(text.into())
            .expect("Template preview already set");
    }

    fn value_to_text(
        value: Value,
        content_type: Option<ContentType>,
    ) -> String {
        match content_type {
            Some(ContentType::Json) => {
                serde_json::to_string_pretty(&value).unwrap()
            }
            None => format!("{value}"),
        }
    }
}

impl Generate for &Preview {
    type Output<'this>
        = Text<'static>
    where
        Self: 'this;

    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this,
    {
        (*self.text()).clone()
    }
}

impl From<Procedure> for Preview {
    fn from(template: Procedure) -> Self {
        Self::new(template, None)
    }
}

impl Widget for &Preview {
    fn render(self, area: Rect, buf: &mut Buffer) {
        (&**self.text()).render(area, buf)
    }
}

/// Text to display in a preview
#[derive(Debug)]
enum PreviewText {
    /// Procedure is static so we can stringify it on construction and just use
    /// that value
    Static(Identified<Text<'static>>),
    /// Render procedure is dynamic. We'll kick off a task to render it and a
    /// callback will set this value. Use [OnceLock::get] to access the value,
    /// and if it's unset show a placeholder.
    Dynamic(Arc<OnceLock<Identified<Text<'static>>>>),
}
