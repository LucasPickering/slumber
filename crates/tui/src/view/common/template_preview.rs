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
use slumber_core::{collection::ValueTemplate, render::TemplateContext};
use slumber_template::{
    Context, LazyValue, RenderedChunk, RenderedOutput, Template, Value,
};
use std::{
    borrow::Cow, convert::Infallible, fmt::Write as _, ops::Deref, str::FromStr,
};
use winnow::{
    ModalResult, Parser,
    ascii::{dec_uint, digit1},
    combinator::{alt, eof, repeat_till},
    stream::Accumulate,
    token::{any, take, take_until},
};

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
                    let preview_string = if can_stream {
                        template.render_preview(&context.stream()).await
                    } else {
                        template.render_preview(&context).await
                    };
                    let text = preview_string.into_text();

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

    /// TODO move this into a different trait?
    async fn render_preview<Ctx: Context>(
        &self,
        context: &Ctx,
    ) -> PreviewString;
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
    ) -> PreviewString {
        let output = self.render(context).await;
        let mut rendered = PreviewString::new();
        for chunk in output {
            rendered.push_chunk(chunk);
        }
        rendered
    }
}

/// TODO
pub struct PreviewString(String);

impl PreviewString {
    // TODO
    const SENTINEL: &str = "__slumber";
    const SEPARATOR: &str = ":";
    const RENDERED_VARIANT: &str = "result";
    const ERROR_VARIANT: &str = "error";

    /// TODO
    pub async fn render_value_template<Ctx: Context>(
        context: &Ctx,
        template: &ValueTemplate,
        encoder: impl FnOnce(Value) -> String,
    ) -> Self {
        let output = template.render(context).await;
        // TODO add in-band tags
        let value = match output.unpack() {
            LazyValue::Value(value) => value,
            LazyValue::Stream { source, stream } => todo!(),
            LazyValue::Nested(output) => todo!(),
        };
        // Use the given encoding (e.g. JSON) to convert the value to a string
        Self(encoder(value))
    }

    fn new() -> Self {
        Self(String::new())
    }

    /// TODO
    fn push_chunk(&mut self, chunk: RenderedChunk) {
        match chunk {
            RenderedChunk::Raw(s) => self.0.push_str(&s),
            RenderedChunk::Rendered(lazy) => {
                let chunk_text = match lazy {
                    LazyValue::Value(value) => {
                        // We could potentially use MaybeStr to show binary
                        // data as hex, but that could get weird if there's
                        // text data in the template as well. This is
                        // simpler and prevents giant binary blobs from
                        // getting rendered in.
                        value
                            .try_into_string()
                            .unwrap_or_else(|_| "<binary>".into())
                    }
                    LazyValue::Stream { source, .. } => {
                        format!("<{source}>")
                    }
                    // Stringify all the nested chunks and concat them
                    // together. Nested chunks can
                    // be generated by a profile field. This
                    // whole thing will get styled as dynamic, even if it
                    // contains raw chunks within.
                    LazyValue::Nested(output) => {
                        todo!()
                    }
                };
                self.push_sentinel(Self::RENDERED_VARIANT, &chunk_text);
            }
            RenderedChunk::Error(error) => {
                self.push_sentinel(Self::ERROR_VARIANT, &error.to_string());
            }
        }
    }

    /// TODO
    fn push_sentinel(&mut self, variant: &str, content: &str) {
        write!(
            &mut self.0,
            "{sentinel}:{variant}:{len}:{content}",
            sentinel = Self::SENTINEL,
            len = content.len(),
        )
        .unwrap();
    }

    /// TODO
    fn into_text(self) -> Text<'static> {
        fn parse_chunk<'a>(
            input: &mut &'a str,
        ) -> Result<(ChunkKind, &'a str), winnow::error::ErrMode<()>> {
            let (_, _, variant, _, size, _) = (
                PreviewString::SENTINEL,
                PreviewString::SEPARATOR,
                take_until(1.., PreviewString::SEPARATOR).parse_to(),
                PreviewString::SEPARATOR,
                dec_uint::<_, usize, _>,
                PreviewString::SEPARATOR,
            )
                .parse_next(input)?;
            // TODO take bytes, not chars
            let content = take(size).parse_next(input)?;
            Ok((variant, content))
        }

        let styles = ViewContext::styles().template_preview;
        let mut parse_styles =
            |input: &mut &str| -> ModalResult<TextBuilder, ()> {
                repeat_till(
                    0..,
                    alt((
                        take_until(0.., Self::SENTINEL)
                            .map(|s| (s, Style::default())),
                        parse_chunk.map(|(variant, content)| {
                            let style = match variant {
                                ChunkKind::Rendered => styles.text,
                                ChunkKind::Error => styles.error,
                            };
                            (content, style)
                        }),
                    )),
                    eof,
                )
                .map(|(builder, _)| builder)
                .parse_next(input)
            };

        parse_styles.parse(self.0.as_str()).expect("TODO").build()
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
}

impl TextBuilder {
    fn new() -> Self {
        Self {
            lines: vec![Line::default()],
        }
    }

    /// Add rendered chunks to the text
    ///
    /// For [Template], this is the only thing required to build the preview.
    fn add_chunks(&mut self, output: RenderedOutput) {
        let styles = ViewContext::styles();

        // Each chunk will get its own styling, but we can't just make each
        // chunk a Span, because one chunk might have multiple lines. And we
        // can't make each chunk a Line, because multiple chunks might be
        // together on the same line. So we need to walk down each line and
        // manually split the lines
        for chunk in output {
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

    fn build(self) -> Text<'static> {
        Text::from_iter(self.lines)
    }
}

/// TODO
impl Accumulate<(&str, Style)> for TextBuilder {
    fn initial(_capacity: Option<usize>) -> Self {
        Self::new()
    }

    fn accumulate(&mut self, (text, style): (&str, Style)) {
        self.add_text_styled(text, style);
    }
}

// TODO move string literals down here
enum ChunkKind {
    Rendered,
    Error,
}

impl FromStr for ChunkKind {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            PreviewString::RENDERED_VARIANT => Ok(Self::Rendered),
            PreviewString::ERROR_VARIANT => Ok(Self::Error),
            _ => Err(()),
        }
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
            Line::from(dynamic("ww")),
            Line::from(dynamic("🧡")),
            Line::from(vec![
                dynamic("💛"),
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
        vec![Line::from(vec![Span::raw("binary data: "), dynamic("<binary>")])]
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

        let output = template.render(&context).await;
        let mut builder = TextBuilder::new();
        builder.add_chunks(output);
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
        vec!["hello ".into(), dynamic("bob")],
    )]
    #[case::stream(
        "data: {{ command(['echo', 'test']) }}",
        vec!["data: ".into(), dynamic("<command `echo test`>")])]
    #[case::stream_profile(
        "data: {{ stream }}",
        vec!["data: ".into(), dynamic("<command `echo test`>")],
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

        let text = template.render_preview(&context.stream()).await.into_text();
        let expected = Text::from(Line::from(expected));
        assert_eq!(text, expected);
    }

    /// Preview JSON templates as text. This tests:
    /// - Primitive values
    /// - Template strings: unpacked where possible, nested chunks where not
    /// - Collections: newlines, separators, and indentation
    /// - Error chunks
    /// - Dynamic chunks are styled as such
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
        let text = PreviewString::render_value_template(
            &context,
            &json_template,
            |value| serde_json::to_string_pretty(&value).unwrap(),
        )
        .await
        .into_text();

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
                    dynamic("Ted"),
                    "!".into(),
                    "\"".into(),
                ],
                true,
            ),
            field(1, "unpacked_template", vec![dynamic("3")], true),
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
    fn dynamic(text: &str) -> Span<'_> {
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
