//! Utilities for rendering templates to styled previews

mod text_builder;

use crate::view::util::preview::text_builder::{ChunkTag, TextBuilder};
use async_trait::async_trait;
use futures::future;
use ratatui::text::Text;
use serde::Serialize;
use slumber_core::{
    collection::ValueTemplate,
    util::json::{JsonTemplateError, YamlTemplateError},
};
use slumber_template::{
    Context, LazyValue, RenderedChunk, RenderedChunks, Template, Value,
};
use std::{
    borrow::Cow,
    cell::Cell,
    fmt::Debug,
    io::{self, Write},
    str::FromStr,
};

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
        let chunks = self.render(context).await;
        // Stitch the output together into Text
        TextBuilder::from_chunks(&chunks).build()
    }
}

/// A previewable wrapper of [ValueTemplate] for JSON bodies
#[derive(Clone, Debug, PartialEq)]
pub struct JsonTemplate(pub ValueTemplate);

impl FromStr for JsonTemplate {
    type Err = JsonTemplateError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        ValueTemplate::parse_json(s).map(Self)
    }
}

#[async_trait(?Send)]
impl Preview for JsonTemplate {
    fn display(&self) -> Cow<'_, str> {
        // Serialize with serde_json so we can offload formatting
        serde_json::to_string_pretty(&self.0)
            // There are no ValueTemplate values that fail to serialize
            .expect("Template to JSON conversion cannot fail")
            .into()
    }

    fn is_dynamic(&self) -> bool {
        self.0.is_dynamic()
    }

    async fn render_preview<Ctx: Context>(
        &self,
        context: &Ctx,
    ) -> Text<'static> {
        let value = PreviewValue::render(&self.0, context).await;
        let mut injector = StyleInjector::default();
        serde_json::to_writer_pretty(&mut injector, &value)
            .expect("PreviewValue serialization cannot fail");
        TextBuilder::from_tagged(&injector.buffer).build()
    }
}

/// A previewable wrapper of [ValueTemplate] for profile fields
///
/// This displays/edits values as YAML, because that's how they're written in
/// the collection file. Technically we could use any format here, as these
/// fields are never directly serialized into requests, they're only used to
/// build other values.
#[derive(Clone, Debug, PartialEq)]
pub struct YamlTemplate(pub ValueTemplate);

impl FromStr for YamlTemplate {
    type Err = YamlTemplateError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // First, parse it as regular YAML
        let yaml: serde_yaml::Value = serde_yaml::from_str(s)?;
        // Then map all the strings as templates
        let mapped = yaml.try_into()?;
        Ok(Self(mapped))
    }
}

#[async_trait(?Send)]
impl Preview for YamlTemplate {
    fn display(&self) -> Cow<'_, str> {
        // Serialize with serde_yaml so we can offload formatting
        let mut s = serde_yaml::to_string(&self.0)
            // There are no ValueTemplate values that fail to serialize
            .expect("Template to YAML conversion cannot fail");
        // YAML includes a trailing newline that is not helpful
        trim_newline(&mut s);
        s.into()
    }

    fn is_dynamic(&self) -> bool {
        self.0.is_dynamic()
    }

    async fn render_preview<Ctx: Context>(
        &self,
        context: &Ctx,
    ) -> Text<'static> {
        let value = PreviewValue::render(&self.0, context).await;
        let mut injector = StyleInjector::default();
        serde_yaml::to_writer(&mut injector, &value)
            .expect("PreviewValue serialization cannot fail");
        // YAML includes a trailing newline that is not helpful
        trim_newline(&mut injector.buffer);
        TextBuilder::from_tagged(&injector.buffer).build()
    }
}

/// Strip the trailing newline from a YAML string
///
/// YAML always includes a trailing newline, even for primitive values like
/// `null`. This causes unnecessary multi-line display in the TUI, and doesn't
/// provide any value.
fn trim_newline(yaml: &mut String) {
    // Usually it's the last character, but if this is happening after style
    // tagging, it's possible something else ended up behind it. So remove the
    // final newline, wherever it is
    if let Some(index) = yaml.rfind('\n') {
        yaml.remove(index);
    }
}

/// A complex value rendered from a [ValueTemplate]
///
/// This is like a [Value], except:
/// - It can hold errors. Failed renders are not fatal. Instead, the errors are
///   stored where they occurred so the rest of the render can proceed.
/// - The provenance of values (raw vs dynamic) is stored, so styling can be
///   applied appropriately
///
/// Previews are really complicated to render because we need to carry over
/// styling information. It's done in several phases:
///
/// 1. [ValueTemplate]: The unrendered template
///   1. Rendering
/// 2. [PreviewValue]: The rendered value, with errors and provenance retained
///   1. Serialization. Style information is serialized within the content so it
///      can be parsed back out in the next step.
/// 3. [String]: The serialized text (JSON, YAML, etc.)
///   1. Text construction & style parsing
/// 4. [Text]: The styled text
///
/// This whole charade is necessary in order to re-use `serde_json`/`serde_yaml`
/// for step 2->3. It seems like a lot of code (and it is), but it would be a
/// lot worse to re-implement that serialization. This is also much more
/// scalable, because each new serialization format only requires a small amount
/// of new work, instead of having to write an entire formatter.
///
/// The construction of this relies on a key fact:
///
/// A raw value can contain dynamic values, but a dynamic value *cannot* contain
/// raw values. For example, here's a template that renders to a [PreviewValue]
/// that is partially raw, partially dynamic:
///
/// ```yaml
/// data:
///   static: 3
///   inner:
///     static: 4
///     dynamic: "{{ 5 }}"
///     stitched: "after 5 comes {{ 6 }}"
/// ```
///
/// Once rendered, this is:
///
/// ```yaml
/// data:
///   static: 3
///   inner:
///     static: 4
///     dynamic: 5
///     stitched: "after 5 comes 6" # `after 5 comes ` is static, `6` is dynamic
/// ```
#[derive(Debug)]
enum PreviewValue {
    /// A value defined literally in source
    Raw(RawValue),
    /// A value computed dynamically from a template chunk
    Dynamic(Value),
}

impl PreviewValue {
    /// Render from a [ValueTemplate]
    async fn render<Ctx: Context>(
        template: &ValueTemplate,
        context: &Ctx,
    ) -> PreviewValue {
        match template {
            ValueTemplate::Null => PreviewValue::Raw(RawValue::Null),
            ValueTemplate::Boolean(b) => {
                PreviewValue::Raw(RawValue::Boolean(*b))
            }
            ValueTemplate::Integer(i) => {
                PreviewValue::Raw(RawValue::Integer(*i))
            }
            ValueTemplate::Float(f) => PreviewValue::Raw(RawValue::Float(*f)),
            ValueTemplate::String(template) => {
                let chunks = template.render(context).await;
                Self::unpack_chunks(chunks)
            }
            ValueTemplate::Array(array) => {
                let items = future::join_all(
                    array.iter().map(|value| Self::render(value, context)),
                )
                .await;
                PreviewValue::Raw(RawValue::Array(items))
            }
            ValueTemplate::Object(object) => {
                let entries =
                    future::join_all(object.iter().map(|(key, value)| async {
                        let key = PreviewChunks(
                            key.render(context).await.into_chunks(),
                        );
                        let value = Self::render(value, context).await;
                        (key, value)
                    }))
                    .await;
                PreviewValue::Raw(RawValue::Object(entries))
            }
        }
    }

    /// Unpack a list of chunks into a preview value
    ///
    /// If the list is a single chunk, unpack its value. Otherwise, store the
    /// list of chunks together so they can be concatenated (with styling)
    /// during serialization.
    fn unpack_chunks(chunks: RenderedChunks) -> Self {
        fn string(chunks: Vec<RenderedChunk>) -> PreviewValue {
            PreviewValue::Raw(RawValue::String(PreviewChunks(chunks)))
        }

        match <[_; 1]>::try_from(chunks.into_chunks()) {
            Ok(chunks @ [RenderedChunk::Raw(_)]) => string(chunks.into()),
            Ok([RenderedChunk::Dynamic(LazyValue::Value(value))]) => {
                PreviewValue::Dynamic(value)
            }
            // Error can't be unpacked because it has to be written as a string
            Ok(
                chunks @ [
                    RenderedChunk::Dynamic(LazyValue::Stream { .. })
                    | RenderedChunk::Error(_),
                ],
            ) => string(chunks.into()),
            // I don't think this case is actually possible because we
            // didn't unpack anywhere. Flaw in the type design!!
            Ok([RenderedChunk::Dynamic(LazyValue::Nested(chunks))]) => {
                string(chunks.into_chunks())
            }
            // There's multiple chunks, we have to stitch them together
            Err(chunks) => string(chunks),
        }
    }
}

impl Serialize for PreviewValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            PreviewValue::Raw(raw_value) => raw_value.serialize(serializer),
            // Tag the entire value as dynamic
            PreviewValue::Dynamic(value) => StyleInjector::with_tag(
                || value.serialize(serializer),
                ChunkTag::Dynamic,
            ),
        }
    }
}

/// [Value] that was defined literally, but may contain dynamic values within
///
/// This is mutually recursive with [PreviewValue].
#[derive(Debug, Serialize)]
#[serde(untagged)]
enum RawValue {
    Null,
    Boolean(bool),
    Integer(i64),
    Float(f64),
    String(PreviewChunks),
    Array(Vec<PreviewValue>),
    /// Object is stored as a list instead of map because the key is not
    /// hashable, and we don't care about lookup
    #[serde(serialize_with = "slumber_util::serialize_mapping")]
    Object(Vec<(PreviewChunks, PreviewValue)>),
}

/// A wrapper of rendered chunks, ready to be serialized
///
/// This injects inline style information into the serialized text, which will
/// be parsed by [TextBuilder].
#[derive(Debug)]
struct PreviewChunks(Vec<RenderedChunk>);

impl Serialize for PreviewChunks {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        // Combine the chunks into a single string
        let mut content = String::new();
        for chunk in &self.0 {
            let chunk_kind = match chunk {
                RenderedChunk::Raw(_) => None,
                RenderedChunk::Dynamic(_) => Some(ChunkTag::Dynamic),
                RenderedChunk::Error(_) => Some(ChunkTag::Error),
            };
            let chunk_content = TextBuilder::get_chunk_text(chunk);

            // If there's styling to apply, append it to the content
            if let Some(kind) = chunk_kind {
                kind.push_tagged_content(&mut content, &chunk_content);
            } else {
                content.push_str(&chunk_content);
            }
        }

        content.serialize(serializer)
    }
}

/// [Write] impl for injecting styling metadata into non-text serialized values
///
/// This is a shim between the generic [serde::Serializer] (JSON, YAML, etc.)
/// and [TextBuilder]. The [Serialize] implementation of [PreviewValue] can't
/// directly serialize styling metadata into non-string values, because the
/// serialization formats don't support arbitrary text anywhere. This writer
/// uses a thread-local to let the [Serialize] impl and this writer pass data
/// *around* the serializer. It's then injected into the output byte stream,
/// which is subsequently parsed by [TextBuilder] and reconstructed into styles.
#[derive(Default)]
struct StyleInjector {
    buffer: String,
}

impl StyleInjector {
    thread_local! {
        static VALUE_TAG: Cell<Option<ChunkTag>> = Cell::default();
    }

    /// Call a closure with the thread-local value tag set
    ///
    /// Use this when serializing a stylized value. This uses the thread-local
    /// as an out-of-band channel to communicate from the [Serialize] impl to
    /// the writer that the value needs to be serialized within a [ChunkTag].
    ///
    /// This is used for non-string values, where the serialization format
    /// doesn't support arbitrary text (e.g. wrapping an int or entire object
    /// with styling).
    fn with_tag<T>(f: impl FnOnce() -> T, chunk_kind: ChunkTag) -> T {
        Self::VALUE_TAG.set(Some(chunk_kind));
        let out = f();
        Self::VALUE_TAG.set(None);
        out
    }
}

impl Write for StyleInjector {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let content = str::from_utf8(buf).expect("Text preview must be UTF-8");
        if let Some(tag) = Self::VALUE_TAG.get() {
            tag.push_tagged_content(&mut self.buffer, content);
        } else {
            self.buffer.push_str(content);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::{
        context::ViewContext,
        test_util::{TestHarness, harness},
    };
    use indexmap::indexmap;
    use pretty_assertions::assert_eq;
    use ratatui::text::{Line, Span};
    use rstest::rstest;
    use serde_json::json;
    use slumber_core::{collection::Profile, render::TemplateContext};
    use slumber_util::Factory;

    /// Preview a plain template with:
    /// - Line breaks
    /// - Multi-byte characters
    /// - Binary data
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
    async fn test_preview_template(
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
        let context = TemplateContext::factory(profile);

        let chunks = template.render(&context).await;
        let actual = TextBuilder::from_chunks(&chunks).build();
        assert_eq!(actual, Text::from(expected));
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
        let context = TemplateContext::factory(profile);

        let text = template.render_preview(&context.stream()).await;
        let expected = Text::from(Line::from(expected));
        assert_eq!(text, expected);
    }

    /// Stringify JSON body to a raw template string, for editing
    #[rstest]
    #[case::null(ValueTemplate::Null, "null")]
    #[case::bool(true.into(), "true")]
    #[case::int((-300).into(), "-300")]
    #[case::float((-17.3).into(), "-17.3")]
    // JSON doesn't support inf/NaN so these map to null
    #[case::float_inf(f64::INFINITY.into(), "null")]
    #[case::float_nan(f64::NAN.into(), "null")]
    // Template is parsed and re-stringified
    #[case::template("{{www}}".into(), r#""{{ www }}""#)]
    #[case::array(vec!["{{w}}", "raw"].into(), r#"[
  "{{ w }}",
  "raw"
]"#)]
    #[case::object(
        vec![("{{w}}", "{{x}}")].into(), r#"{
  "{{ w }}": "{{ x }}"
}"#
    )]
    fn test_json_display(
        #[case] template: ValueTemplate,
        #[case] expected: &str,
    ) {
        assert_eq!(JsonTemplate(template).display(), expected);
    }

    /// Preview JSON templates as text. This tests that content is preserved
    /// and styling indicates which parts are dynamic.
    ///
    /// Syntax highlighting is applied by the body component, so we don't have
    /// to worry about it here.
    #[rstest]
    #[case::null(json!(null), "null".into())]
    #[case::int(json!(3), "3".into())]
    #[case::float(json!(4.32), "4.32".into())]
    #[case::bool(json!(false), "false".into())]
    #[case::string(json!("hello"), "\"hello\"".into())]
    #[case::string_escaped(
        json!("i have a \" quote"), r#""i have a \" quote""#.into()
    )]
    #[case::template(
        json!("my name is {{ 'Ted' }}!"),
        // Just the dynamic part is styled colorly like
        line(vec!["\"my name is ".into(), rendered("Ted"), "!\"".into()]),
    )]
    #[case::template_unpacked(json!("{{ 3 }}"), rendered("3").into())]
    #[case::template_escaped(
        // Entire dynamic chunk gets styling. Make sure the escaped quote
        // doesn't cause any off-by-ones
        json!("dynamic: {{ 'with \" quote' }}"),
        line(vec![
            "\"dynamic: ".into(), rendered(r#"with \" quote"#), quote(),
        ]),
    )]
    #[case::error(
        // Error can't be unpacked because it wouldn't be valid JSON
        json!("{{ w }}"), line(vec![quote(), error("Error"), quote()]),
    )]
    #[case::multi_chunk_error(
        json!("error? {{ w }} error!"),
        line(vec!["\"error? ".into(), error("Error"), " error!\"".into()]),
    )]
    #[case::array(
        json!(["dynamic {{ 'string' }}", "error {{ w }}", null]),
        vec![
            "[".into(),
            vec![
                r#"  "dynamic "#.into(), rendered("string"), "\",".into(),
            ].into(),
            vec![r#"  "error "#.into(), error("Error"), "\",".into()].into(),
            "  null".into(),
            "]".into(),
        ].into(),
    )]
    #[expect(clippy::needless_raw_string_hashes)]
    #[case::object(
        json!({
            "a": 1,
            "nested": {
                "b": 2,
                "nested": {"c": [3, 4, 5]},
                "d": "dynamic {{ 'string' }}",
                "e": "error {{ w }}",
            }
        }),
        vec![
            // Raw strings on everything makes the alignment consistent
            "{".into(),
            r#"  "a": 1,"#.into(),
            r#"  "nested": {"#.into(),
            r#"    "b": 2,"#.into(),
            r#"    "nested": {"#.into(),
            r#"      "c": ["#.into(),
            r#"        3,"#.into(),
            r#"        4,"#.into(),
            r#"        5"#.into(),
            r#"      ]"#.into(),
            r#"    },"#.into(),
            vec![
                r#"    "d": "dynamic "#.into(),
                rendered("string"),
                "\",".into(),
            ].into(),
            vec![
                r#"    "e": "error "#.into(),
                error("Error"),
                quote(),
            ].into(),
            r#"  }"#.into(),
            r#"}"#.into(),
        ].into(),
    )]
    // Really these streams should be resolved because JSON bodies don't support
    // streaming, but I got lazy here. Maybe I can fix this if I ever refactor
    // the rendered output types
    #[case::stream(
        json!("stream: {{ command(['echo', 'test']) }}"),
        line(vec![
            "\"stream: ".into(), rendered("<command `echo test`>"), quote(),
        ]),
    )]
    // Stream does *not* get unpacked
    #[case::stream_unpacked(
        json!("{{ command(['echo', 'test']) }}"),
        line(vec![quote(), rendered("<command `echo test`>"), quote()])
    )]
    #[case::nested_dynamic(
        json!({ "double_dynamic": "{{ object }}" }),
        // The entire value is styled as dynamic
        vec![
            "{".into(),
            vec![r#"  "double_dynamic": "#.into(), rendered("{")].into(),
            rendered(r#"    "a": 1,"#).into(),
            rendered(r#"    "b": 2"#).into(),
            rendered("  }").into(),
            "}".into(),
        ].into()
    )]
    #[tokio::test]
    async fn test_json_preview(
        _harness: TestHarness,
        #[case] json: serde_json::Value,
        #[case] expected: Text<'static>,
    ) {
        let json_template: JsonTemplate =
            JsonTemplate(json.try_into().unwrap());

        let profile = Profile {
            data: indexmap! {
                "object".into() => vec![("a", 1), ("b", 2)].into(),
            },
            ..Profile::factory(())
        };
        let context = TemplateContext::factory(profile);
        let text = json_template.render_preview(&context).await;
        assert_eq!(text, expected);
    }

    /// Stringify YAML value to a raw template string, for editing
    #[rstest]
    #[case::null(ValueTemplate::Null, "null")]
    #[case::bool(true.into(), "true")]
    #[case::int((-300).into(), "-300")]
    #[case::float((-17.3).into(), "-17.3")]
    // YAML does support inf/NaN
    #[case::float_inf(f64::INFINITY.into(), ".inf")]
    #[case::float_nan(f64::NAN.into(), ".nan")]
    // Template is parsed and re-stringified
    #[case::template("{{www}}".into(), "'{{ www }}'")]
    #[case::array(vec!["{{w}}", "raw"].into(), "- '{{ w }}'
- raw")]
    #[case::object(
        vec![("{{w}}", "{{x}}")].into(), "'{{ w }}': '{{ x }}'"
    )]
    fn test_yaml_display(
        #[case] template: ValueTemplate,
        #[case] expected: &str,
    ) {
        assert_eq!(YamlTemplate(template).display(), expected);
    }

    /// Preview YAML templates as text.
    ///
    /// The values are specified as JSON because there's no `yaml!` macro, and I
    /// stole all the test cases from the JSON test.
    #[rstest]
    #[case::null(json!(null), "null".into())]
    #[case::int(json!(3), "3".into())]
    #[case::float(json!(4.32), "4.32".into())]
    #[case::bool(json!(false), "false".into())]
    #[case::string(json!("hello"), "hello".into())]
    #[case::string_escaped(
        // We have to do some funky stuff to get the serializer to escape quotes
        json!("{i have \"' quotes}"), "'{i have \"'' quotes}'".into()
    )]
    #[case::template(
        json!("my name is {{ 'Ted' }}!"),
        // Just the dynamic part is styled colorly like
        line(vec!["my name is ".into(), rendered("Ted"), "!".into()]),
    )]
    #[case::template_unpacked(json!("{{ 3 }}"), rendered("3").into())]
    #[case::template_escaped(
        // Entire dynamic chunk gets styling. Make sure the escaped quote
        // doesn't cause any off-by-ones.
        // The {} wrapper forces the YAML serializer to use quotes
        json!("{dynamic: {{ 'with \\' quote' }}}"),
        line(vec![
            "'{dynamic: ".into(), rendered("with '' quote"), "}'".into(),
        ]),
    )]
    #[case::error(json!("{{ w }}"), error("Error").into())]
    #[case::multi_chunk_error(
        json!("error? {{ w }} error!"),
        line(vec!["error? ".into(), error("Error"), " error!".into()]),
    )]
    #[case::array(
        json!(["dynamic {{ 'string' }}", "error {{ w }}", null]),
        vec![
            vec!["- dynamic ".into(), rendered("string")].into(),
            vec!["- error ".into(), error("Error")].into(),
            "- null".into(),
        ].into(),
    )]
    #[case::object(
        json!({
            "a": 1,
            "nested": {
                "b": 2,
                "nested": {"c": [3, 4, 5]},
                "d": "dynamic {{ 'string' }}",
                "e": "error {{ w }}",
            }
        }),
        vec![
            "a: 1".into(),
            "nested:".into(),
            "  b: 2".into(),
            "  nested:".into(),
            "    c:".into(),
            "    - 3".into(),
            "    - 4".into(),
            "    - 5".into(),
            vec!["  d: dynamic ".into(), rendered("string")].into(),
            vec!["  e: error ".into(), error("Error")].into(),
        ].into(),
    )]
    // Really these streams should be resolved because JSON bodies don't support
    // streaming, but I got lazy here. Maybe I can fix this if I ever refactor
    // the rendered output types
    #[case::stream(
        json!("stream: {{ command(['echo', 'test']) }}"),
        line(vec![
            "'stream: ".into(), rendered("<command `echo test`>"), "'".into(),
        ]),
    )]
    // Stream does *not* get unpacked
    #[case::stream_unpacked(
        json!("{{ command(['echo', 'test']) }}"),
        rendered("<command `echo test`>").into()
    )]
    // This is broken because the YAML serializer seems to buffer its output
    // before passing it to the writer. This means the thread-local styling
    // isn't set when the StyleInjector is called. Fortunately it only applies
    // to profile values used within profile values, so not the biggest deal.
    #[ignore = "Styling on nested YAML values is broken"]
    #[case::nested_dynamic(
        json!({ "double_dynamic": "{{ object }}" }),
        // The entire value is styled as dynamic
        vec![
            "double_dynamic:".into(),
            rendered("  a: 1").into(),
            rendered("  b: 2").into(),
        ].into()
    )]
    #[tokio::test]
    async fn test_yaml_preview(
        _harness: TestHarness,
        #[case] json: serde_json::Value,
        #[case] expected: Text<'static>,
    ) {
        // Parsing JSON to a ValueTemplate is the same as converting YAML.
        // There's no yaml! macro or YAML->ValueTemplate converter, so this is
        // just an easier way of defining the test data.
        let yaml_template = YamlTemplate(json.try_into().unwrap());

        let profile = Profile {
            data: indexmap! {
                "object".into() => vec![("a", 1), ("b", 2)].into(),
            },
            ..Profile::factory(())
        };
        let context = TemplateContext::factory(profile);
        let text = yaml_template.render_preview(&context).await;
        assert_eq!(text, expected);
    }

    /// An unstyled `"`
    fn quote() -> Span<'static> {
        "\"".into()
    }

    /// Style some text as rendered
    fn rendered(text: &str) -> Span<'_> {
        Span::styled(text, ViewContext::styles().template_preview.dynamic)
    }

    /// Style some text as an error
    fn error(text: &str) -> Span<'_> {
        Span::styled(text, ViewContext::styles().template_preview.error)
    }

    fn line(spans: Vec<Span<'static>>) -> Text<'static> {
        Line::from_iter(spans).into()
    }
}
