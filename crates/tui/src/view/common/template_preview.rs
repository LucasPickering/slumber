use crate::{
    message::Message,
    view::{
        UpdateContext, ViewContext,
        component::{Component, ComponentId},
        event::{BroadcastEvent, Emitter, Event, EventMatch, ToEmitter},
    },
};
use async_trait::async_trait;
use futures::FutureExt;
use ratatui::{
    style::{Style, Styled},
    text::{Line, Span, Text},
};
use slumber_core::{
    collection::ValueTemplate, render::TemplateContext,
    util::json::value_to_json,
};
use slumber_template::{
    Context, LazyValue, RenderedChunk, RenderedOutput, Template,
};
use std::{borrow::Cow, ops::Deref, str::FromStr};

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
/// [Template], but for non-string values it can be other types. Anything that
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

/// A template that can be rendered to text for preview
///
/// Bounds:
/// - `Clone`: Required to send the template out for preview and retain a copy
/// - `FromStr`: Parse overrides from strings
/// - `PartialEq`: Compare override to original to see if it's changed
#[async_trait(?Send)]
pub trait Preview: 'static + Clone + FromStr + PartialEq {
    /// Get the template's equivalent source code
    ///
    /// This is *functionally* equivalent to the template's input source, but
    /// may not match exactly. For example, insignicant whitespace within a
    /// template expression may be added/lost.
    fn display(&self) -> Cow<'_, str>;

    /// Does the template contain *any* dynamic expressions?
    fn is_dynamic(&self) -> bool;

    /// Render the template as preview text, including styling
    async fn render_preview<Ctx: Context>(
        &self,
        context: &Ctx,
    ) -> Text<'static>;
}

#[async_trait(?Send)]
impl Preview for Template {
    fn display(&self) -> Cow<'_, str> {
        self.display()
    }

    fn is_dynamic(&self) -> bool {
        self.is_dynamic()
    }

    async fn render_preview<Ctx: Context>(
        &self,
        context: &Ctx,
    ) -> Text<'static> {
        let output = self.render(context).await;
        // Stitch the output together into Text
        let mut builder = TextBuilder::new();
        builder.add_chunks(output);
        builder.build()
    }
}

/// Preview a [ValueTemplate] as JSON
pub async fn render_json_preview<Ctx: Context>(
    context: &Ctx,
    template: &ValueTemplate,
) -> Text<'static> {
    /// Recursive helper
    async fn inner<Ctx: Context>(
        context: &Ctx,
        builder: &mut TextBuilder,
        template: &ValueTemplate,
    ) {
        match template {
            ValueTemplate::Null => builder.add_text("null"),
            ValueTemplate::Boolean(false) => builder.add_text("false"),
            ValueTemplate::Boolean(true) => builder.add_text("true"),
            ValueTemplate::Integer(i) => builder.add_text(&i.to_string()),
            ValueTemplate::Float(f) => builder.add_text(&f.to_string()),
            ValueTemplate::String(template) => {
                render_string(context, builder, template).await;
            }
            ValueTemplate::Array(array) => {
                render_collection(
                    builder,
                    array,
                    async |builder, el| {
                        inner(context, builder, el).boxed_local().await;
                    },
                    ("[", "]"),
                    ",",
                )
                .await;
            }
            ValueTemplate::Object(object) => {
                render_collection(
                    builder,
                    object,
                    async |builder, (key, value)| {
                        // Render key. Keys have to be strings, can't unpack
                        builder.add_json_string(key.render(context).await);
                        builder.add_text(": ");
                        // Add the value
                        inner(context, builder, value).boxed_local().await;
                    },
                    ("{", "}"),
                    ",",
                )
                .await;
            }
        }
    }

    let mut builder = TextBuilder::new();
    inner(context, &mut builder, template).await;
    builder.build()
}

/// Generate text for an array/object
async fn render_collection<T>(
    builder: &mut TextBuilder,
    collection: &[T],
    render_fn: impl AsyncFn(&mut TextBuilder, &T),
    (open, close): (&'static str, &'static str),
    separator: &str,
) {
    // Doing this as a hundred little spans seems wasteful, but most of
    // these will get broken apart by the syntax highlighter anyway so
    // it should be minimal cost
    builder.add_text(open);
    builder.new_line();
    builder.indent();
    for (i, element) in collection.iter().enumerate() {
        render_fn(builder, element).await;

        if i < collection.len() - 1 {
            builder.add_text(separator);
        }
        builder.new_line();
    }
    builder.outdent();
    builder.add_text(close);
}

/// Render a string literal preview. Strings *may* unpack to values
async fn render_string<Ctx: Context>(
    context: &Ctx,
    builder: &mut TextBuilder,
    template: &Template,
) {
    let chunks = template.render(context).await;
    match chunks.unpack() {
        // If this unpacks into a value, *don't* include quotes
        LazyValue::Value(value) => {
            let json = value_to_json(value);
            builder.add_text(&format!("{json:#}"));
        }

        LazyValue::Nested(chunks) => {
            // The value can't be unpacked, so it has to be
            // represented as a string
            builder.add_json_string(chunks);
        }
        // I'd love to make this impossible in the type system
        LazyValue::Stream { .. } => {
            unreachable!("JSON bodies don't support streaming")
        }
    }
}

/// A helper to build `Text` from template render output
///
/// This requires some effort because ratatui *loves* line breaks, so we have to
/// very manually construct the text to make sure the structure reflects the
/// line breaks in the input.
///
/// See ratatui docs: <https://docs.rs/ratatui/latest/ratatui/text/index.html>
#[derive(Debug)]
struct TextBuilder {
    lines: Vec<Line<'static>>,
    indent: usize,
}

impl TextBuilder {
    /// Width of an indent, in spaces
    const INDENT_SIZE: usize = 2;

    fn new() -> Self {
        Self {
            lines: vec![Line::default()],
            indent: 0,
        }
    }

    /// Add rendered chunks to the text
    ///
    /// For [Template], this is the only thing required to build the preview.
    fn add_chunks(&mut self, chunks: RenderedOutput) {
        let styles = ViewContext::styles();

        // Each chunk will get its own styling, but we can't just make each
        // chunk a Span, because one chunk might have multiple lines. And we
        // can't make each chunk a Line, because multiple chunks might be
        // together on the same line. So we need to walk down each line and
        // manually split the lines
        for chunk in chunks {
            let style = match chunk {
                RenderedChunk::Raw(_) => Style::default(),
                RenderedChunk::Rendered { .. } => styles.template_preview.text,
                RenderedChunk::Error(_) => styles.template_preview.error,
            };
            let chunk_text = Self::get_chunk_text(chunk);

            self.add_text_styled(&chunk_text, style);
        }
    }

    /// Append some plain text to the builder
    ///
    /// The text will be split on newline as appropriate, but *no* additional
    /// line breaks will be added.
    fn add_text(&mut self, text: &str) {
        self.add_text_styled(text, Style::default());
    }

    /// Append some plain text to the builder with some style
    ///
    /// The text will be split on newline as appropriate, but *no* additional
    /// line breaks will be added.
    fn add_text_styled(&mut self, text: &str, style: Style) {
        // The first line should extend the final line of the current text,
        // because there isn't necessarily a line break between chunks
        let mut lines = text.lines();
        if let Some(first_line) = lines.next()
            && !first_line.is_empty()
        {
            self.add_span(Span::styled(first_line.to_owned(), style));
        }

        // Add remaining lines
        for line in lines {
            self.new_line();
            // Don't add empty spans
            if !line.is_empty() {
                self.add_span(Span::styled(line.to_owned(), style));
            }
        }

        // std::lines throws away trailing newlines, but we care about them
        // because the next chunk needs to go on a new line. We also care about
        // keeping trailing newlines at the end of HTTP bodies, for correctness
        if text.ends_with('\n') {
            self.new_line();
        }
    }

    fn add_span(&mut self, span: Span<'static>) {
        let line = self.lines.last_mut().expect("Lines cannot be empty");

        // Add indentation
        if line.spans.is_empty() && self.indent > 0 {
            line.push_span(str::repeat(" ", self.indent * Self::INDENT_SIZE));
        }

        line.push_span(span);
    }

    fn new_line(&mut self) {
        self.lines.push(Line::default());
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

    /// Add a JSON string with quotes to the text
    fn add_json_string(&mut self, chunks: RenderedOutput) {
        self.add_text("\"");
        self.add_chunks(chunks);
        self.add_text("\"");
    }

    /// Increment the indentation level
    fn indent(&mut self) {
        self.indent += 1;
    }

    /// Decrement the indentation level
    fn outdent(&mut self) {
        self.indent -= 1;
    }

    fn build(self) -> Text<'static> {
        Text::from_iter(self.lines)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::test_util::{TestHarness, harness};
    use indexmap::{IndexMap, indexmap};
    use pretty_assertions::assert_eq;
    use rstest::rstest;
    use serde_json::json;
    use slumber_core::{
        collection::{Collection, Profile},
        render::TemplateContext,
        test_util::by_id,
    };
    use slumber_template::Template;
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
    async fn test_build_text(
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
        let mut builder = TextBuilder::new();
        builder.add_chunks(chunks);
        assert_eq!(builder.build(), Text::from(expected));
    }

    /// Preview raw bodies. This tests:
    /// - Plain text
    /// - Dynamic chunks
    /// - Streams
    /// - Streams via profile fields (ensure context is forwarded)
    #[rstest]
    #[case::plain("hello", vec!["hello".into()])]
    #[case::dynamic(
        "hello {{ name }}",
        vec!["hello ".into(), rendered("bob")],
    )]
    #[case::stream(
        "data: {{ command(['echo', 'test']) }}",
        vec!["data: ".into(), rendered("<command `echo test`>")])]
    #[case::stream_profile(
        "data: {{ stream }}",
        vec!["data: ".into(), rendered("<command `echo test`>")],
    )]
    #[tokio::test]
    async fn test_preview_raw(
        _harness: TestHarness,
        #[case] template: Template,
        #[case] expected: Vec<Span<'static>>,
    ) {
        let profile_data = indexmap! {
            "name".into() => "bob".into(),
            "stream".into() => "{{ command(['echo', 'test']) }}".into(),
        };
        let profile = Profile {
            data: profile_data,
            ..Profile::factory(())
        };
        let context = TemplateContext {
            ..TemplateContext::factory((by_id([profile]), IndexMap::default()))
        };

        let text = template.render_preview(&context.stream()).await;
        let expected = Text::from(Line::from(expected));
        assert_eq!(text, expected);
    }

    /// Preview JSON templates as text. This tests:
    /// - Primitive values
    /// - Template strings: unpacked where possible, nested chunks where not
    /// - Collections: newlines, separators, and indentation
    /// - Error chunks
    #[rstest]
    #[tokio::test]
    async fn test_preview_json(_harness: TestHarness) {
        let json = json!({
            "null": null,
            "int": 3,
            "float": 4.32,
            "bool": false,
            "string": "hello",
            "template": "my name is {{ 'Ted' }}!",
            "unpacked_template": "{{ 3 }}",
            "error": "{{ w }}",
            "multi_chunk_error": "error? {{ w }} error!",
            "object": {
                "a": 1,
                "nested": {
                    "b": 2,
                    "nested": {"c": [3, 4, 5]}
                }
            }
        });
        let json_template: ValueTemplate = json.try_into().unwrap();
        let context = TemplateContext::factory(());
        let text = render_json_preview(&context, &json_template).await;

        // Syntax highlighting is applied outside this component, so we don't
        // have to worry about it here
        let expected = vec![
            "{".into(),
            field(1, "null", vec!["null".into()], true),
            field(1, "int", vec!["3".into()], true),
            field(1, "float", vec!["4.32".into()], true),
            field(1, "bool", vec!["false".into()], true),
            field(
                1,
                "string",
                vec!["\"".into(), "hello".into(), "\"".into()],
                true,
            ),
            field(
                1,
                "template",
                // Just the dynamic part is styled colorly like
                vec![
                    "\"".into(),
                    "my name is ".into(),
                    rendered("Ted"),
                    "!".into(),
                    "\"".into(),
                ],
                true,
            ),
            field(1, "unpacked_template", vec!["3".into()], true),
            field(
                1,
                "error",
                vec!["\"".into(), error("Error"), "\"".into()],
                true,
            ),
            field(
                1,
                "multi_chunk_error",
                vec![
                    "\"".into(),
                    "error? ".into(),
                    error("Error"),
                    " error!".into(),
                    "\"".into(),
                ],
                true,
            ),
            field(1, "object", vec!["{".into()], false),
            field(2, "a", vec!["1".into()], true),
            field(2, "nested", vec!["{".into()], false),
            field(3, "b", vec!["2".into()], true),
            field(3, "nested", vec!["{".into()], false),
            field(4, "c", vec!["[".into()], false),
            vec![indent(5), "3".into(), ",".into()].into(),
            vec![indent(5), "4".into(), ",".into()].into(),
            vec![indent(5), "5".into()].into(),
            vec![indent(4), "]".into()].into(),
            vec![indent(3), "}".into()].into(),
            vec![indent(2), "}".into()].into(),
            vec![indent(1), "}".into()].into(),
            "}".into(),
        ]
        .into();
        assert_eq!(text, expected);
    }

    /// Style some text as rendered
    fn rendered(text: &str) -> Span<'_> {
        Span::styled(text, ViewContext::styles().template_preview.text)
    }

    /// Style some text as an error
    fn error(text: &str) -> Span<'_> {
        Span::styled(text, ViewContext::styles().template_preview.error)
    }

    /// Build a text line for a JSON object field
    fn field(
        num_indent: usize,
        name: &'static str,
        value: Vec<Span<'static>>,
        trailing_comma: bool,
    ) -> Line<'static> {
        let mut spans: Vec<Span<'static>> = vec![
            indent(num_indent),
            "\"".into(),
            name.into(),
            "\"".into(),
            ": ".into(),
        ];
        spans.extend(value);
        if trailing_comma {
            spans.push(",".into());
        }
        spans.into()
    }

    fn indent(n: usize) -> Span<'static> {
        str::repeat(" ", n * 2).into()
    }
}
