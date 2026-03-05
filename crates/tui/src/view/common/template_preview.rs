use crate::{
    message::Message,
    view::{
        UpdateContext, ViewContext,
        component::{Component, ComponentId},
        event::{BroadcastEvent, Emitter, Event, EventMatch, ToEmitter},
        util::preview::Preview,
    },
};
use futures::FutureExt;
use ratatui::{
    style::{Style, Styled},
    text::Text,
};
use slumber_core::render::TemplateContext;

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
///
/// `T` is the type of the template being previewed. In most cases, this is
/// `Template`, but for non-string values it can be other types. Anything that
/// implements [Preview] is eligible.
#[derive(Debug)]
pub struct TemplatePreview<T> {
    id: ComponentId,
    /// Template being rendered
    ///
    /// We have to hang onto this so we can re-render if there's a refresh
    /// event
    template: T,
    /// Emitter for events whenever new text is rendered
    emitter: Emitter<TemplatePreviewEvent>,
    /// Does this component of the recipe support streaming? If so, the
    /// template will be rendered to a stream if possible and its metadata will
    /// be displayed rather than the resolved value.
    can_stream: bool,
    /// Is the text a user-given override? This changes the styling
    is_override: bool,
}

impl<T: Preview> TemplatePreview<T> {
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
        template: T,
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
            Text::styled(slf.template.display().into_owned(), style);

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
            let template = self.template.clone();
            let can_stream = self.can_stream;

            // Build a callback that gets the context and uses it to render.
            // This will be spawned into a background task automatically.
            let callback = move |context: TemplateContext| {
                async move {
                    // Render chunks to text
                    let text = if can_stream {
                        template.render_preview(&context.stream()).await
                    } else {
                        template.render_preview(&context).await
                    };

                    // Apply final styling based on override context
                    let text = text.set_style(style);

                    // We can emit the event directly from the callback because
                    // the task is run on a local set
                    emitter.emit(TemplatePreviewEvent(text));
                }
                .boxed_local()
            };

            ViewContext::push_message(Message::TemplatePreview {
                callback: Box::new(callback),
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

impl<T> Component for TemplatePreview<T>
where
    T: 'static + Preview + Clone + PartialEq,
{
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

impl<T> ToEmitter<TemplatePreviewEvent> for TemplatePreview<T> {
    fn to_emitter(&self) -> Emitter<TemplatePreviewEvent> {
        self.emitter
    }
}

/// Emitted event from [TemplatePreview] containing rendered text for a template
#[derive(Debug)]
pub struct TemplatePreviewEvent(pub Text<'static>);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::test_util::{TestHarness, harness};
    use rstest::rstest;
    use slumber_template::Template;
    use slumber_util::assert_matches;

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
}
