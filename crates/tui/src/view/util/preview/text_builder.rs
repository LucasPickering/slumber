use crate::view::context::ViewContext;
use ratatui::{
    style::Style,
    text::{Line, Span, Text},
};
use slumber_template::{LazyValue, RenderedChunk, RenderedChunks};
use std::{
    fmt::{Debug, Write as _},
    mem,
    ops::Deref,
};
use tracing::warn;
use winnow::{
    ModalResult, Parser,
    combinator::{alt, eof, peek, repeat_till, seq},
    token::{any, take_until},
};

/// A helper to build `Text` from template render output
///
/// This requires some effort because ratatui *loves* line breaks, so we have to
/// very manually construct the text to make sure the structure reflects the
/// line breaks in the input.
///
/// See ratatui docs: <https://docs.rs/ratatui/latest/ratatui/text/index.html>
///
/// In addition to joining regular template chunks, this can also parse out
/// style metadata from serialized JSON/YAML/etc. data. See [Self::from_tagged].
#[derive(Debug)]
pub struct TextBuilder {
    lines: Vec<Line<'static>>,
}

impl TextBuilder {
    /// Build preview text from a list of rendered `Template` chunks
    ///
    /// For `Template`, this is the only thing required to build the preview.
    pub fn from_chunks(chunks: &RenderedChunks) -> Self {
        let mut builder = Self::new();
        let styles = ViewContext::styles();

        // Each chunk will get its own styling, but we can't just make each
        // chunk a Span, because one chunk might have multiple lines. And we
        // can't make each chunk a Line, because multiple chunks might be
        // together on the same line. So we need to walk down each line and
        // manually split the lines
        for chunk in chunks {
            let style = match chunk {
                RenderedChunk::Raw(_) => Style::default(),
                RenderedChunk::Dynamic { .. } => {
                    styles.template_preview.dynamic
                }
                RenderedChunk::Error(_) => styles.template_preview.error,
            };
            let chunk_text = Self::get_chunk_text(chunk);

            builder.add_text_styled(&chunk_text, style);
        }

        builder
    }

    /// Parse styling metadata from a serialized (JSON/YAML/etc.) string
    ///
    /// See `PreviewValue` for detailed information about this process. This
    /// function is the final step, loading style metadata from a serialized
    /// string and building user-facing text from it.
    pub fn from_tagged(s: &str) -> Self {
        // Parse out style metadata from the text
        let chunks = parse_tagged_chunks(s);
        let mut builder = Self::new();

        let styles = ViewContext::styles().template_preview;
        for (content, chunk_style) in chunks {
            let style = match chunk_style {
                Some(ChunkTag::Dynamic) => styles.dynamic,
                Some(ChunkTag::Error) => styles.error,
                None => Style::default(),
            };

            builder.add_text_styled(content, style);
        }

        builder
    }

    fn new() -> Self {
        Self {
            lines: vec![Line::default()],
        }
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
            self.add_span(first_line, style);
        }

        // Add remaining lines
        for line in lines {
            self.new_line();
            // Don't add empty spans
            if !line.is_empty() {
                self.add_span(line, style);
            }
        }

        // std::lines throws away trailing newlines, but we care about them
        // because the next chunk needs to go on a new line. We also care about
        // keeping trailing newlines at the end of HTTP bodies, for correctness
        if text.ends_with('\n') {
            self.new_line();
        }
    }

    /// Add a span to the end of the last line
    fn add_span(&mut self, text: &str, style: Style) {
        let line = self.lines.last_mut().expect("Lines cannot be empty");
        // If the styling matches the last span in the text, extend that span
        // instead of adding a new one. This makes testing easier and should
        // cut down on allocations
        if let Some(last_span) = line.spans.last_mut()
            && last_span.style == style
        {
            // The content is probably already owned
            let mut content = mem::take(&mut last_span.content).into_owned();
            content.push_str(text);
            last_span.content = content.into();
        } else {
            line.push_span(Span::styled(text.to_owned(), style));
        }
    }

    /// Add a new blank line to the end
    fn new_line(&mut self) {
        self.lines.push(Line::default());
    }

    /// Get the renderable text for a chunk of a template. This will clone the
    /// text out of the chunk, because it's all stashed behind Arcs
    pub fn get_chunk_text(chunk: &RenderedChunk) -> String {
        match chunk {
            RenderedChunk::Raw(text) => text.deref().into(),
            RenderedChunk::Dynamic(lazy) => match lazy {
                LazyValue::Value(value) => {
                    // We could potentially use MaybeStr to show binary data as
                    // hex, but that could get weird if there's text data in the
                    // template as well. This is simpler and prevents giant
                    // binary blobs from getting rendered in.
                    value
                        .clone() // This is a bit ugly but I'm tired right now
                        .try_into_string()
                        .unwrap_or_else(|_| "<binary>".into())
                }
                LazyValue::Stream { source, .. } => {
                    format!("<{source}>")
                }
            },
            // There's no good way to render the entire error inline
            RenderedChunk::Error(_) => "Error".into(),
        }
    }

    /// Finalize the [Text]
    pub fn build(self) -> Text<'static> {
        Text::from_iter(self.lines)
    }
}

/// Parse a tagged string into tagged/untagged chunks
///
/// Parses this:
///
/// ```notrust
/// my name is $__slumber$dy$Ted$slumber__$ and
/// I am $__slumber$er$Error$slumber__$ years old
/// ```
///
/// into this:
///
/// ```notrust
/// [
///     ("my name is ", None), ("Ted", Dynamic), (" and I am ", None),
///     ("Error", Error), " years old"
/// ]
/// ```
fn parse_tagged_chunks(input: &str) -> Vec<(&str, Option<ChunkTag>)> {
    fn styled_chunk<'i>(
        input: &mut &'i str,
    ) -> ModalResult<(&'i str, ChunkTag), ()> {
        // The tag contains the style and the number of subsequent bytes to
        // which that style applies. It looks like:
        // $__slumber$<tag>$<content>$slumber__$
        let (kind, content): (ChunkTag, &str) = seq!(
            _: ChunkTag::PRELUDE,
            alt((
                ChunkTag::DYNAMIC.value(ChunkTag::Dynamic),
                ChunkTag::ERROR.value(ChunkTag::Error),
            )),
            take_until(0.., ChunkTag::TERMINATOR),
            _: ChunkTag::TERMINATOR,
        )
        .parse_next(input)?;
        Ok((content, kind))
    }

    // Parse an alternating series of raw and styled chunks
    repeat_till(
        0..,
        alt((
            // Styled chunk has to go first, so that if there's no content
            // before the first prelude, we don't get an empty chunk
            styled_chunk.map(|(s, style)| (s, Some(style))),
            // Parse raw content until we hit either a styled chunk or the end
            // of input. It'd be nice to use take_until, but hitting the
            // prelude doesn't necessarily mean we hit a full styled chunk.
            // This is pretty ugly but I'm tired and want to move on.
            repeat_till::<_, _, (), _, _, _, _>(
                1..,
                any,
                alt((peek(styled_chunk).void(), eof.void())),
            )
            .take()
            .map(|s| (s, None)),
        )),
        eof,
    )
    .map(|(chunks, rest)| {
        // We're repeating until eof, so there must be nothing left
        debug_assert_eq!(rest, "", "Remainder must be empty");
        chunks
    })
    .parse(input)
    // The parser *should* never fail because anything invalid is just treated
    // as the raw text, but I'm not willing to bet on it enough to panic here
    .unwrap_or_else(|_| {
        warn!(input, "Failed to parse styled text");
        vec![(input, None)]
    })
}

/// Metadata attached to a chunk of preview text
///
/// This describes how the chunk should be styled when rendered in the TUI. It's
/// serialized as special recognizable text within the overall rendered content,
/// and parsed out by [TextBuilder::from_tagged] to be converted into Ratatui
/// styles.
#[derive(Copy, Clone, Debug)]
#[cfg_attr(test, derive(PartialEq))]
pub enum ChunkTag {
    Dynamic,
    Error,
}

impl ChunkTag {
    const PRELUDE: &str = "$__slumber$";
    const TERMINATOR: &str = "$slumber__$";
    const DYNAMIC: &str = "dy$";
    const ERROR: &str = "er$";

    /// Push some content into a string buffer, wrapper with tags indicating its
    /// chunk type
    ///
    /// The embedded tags can be parsed by [TextBuilder::from_tagged]
    pub fn push_tagged_content(self, buf: &mut String, content: &str) {
        write!(
            buf,
            "{prelude}{tag}{content}{terminator}",
            prelude = Self::PRELUDE,
            tag = match self {
                ChunkTag::Dynamic => Self::DYNAMIC,
                ChunkTag::Error => Self::ERROR,
            },
            terminator = Self::TERMINATOR,
        )
        .expect("Writing to String is infallible");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case::empty("", &[])]
    #[case::untagged("raw", &[("raw", None)])]
    #[case::tagged_only(
        "$__slumber$dy$Test$slumber__$",
        &[("Test", Some(ChunkTag::Dynamic))],
    )]
    #[case::tagged_multiple(
        "dynamic $__slumber$dy$Test$slumber__$ \
        error $__slumber$er$Error$slumber__$ done",
        &[
            ("dynamic ", None),
            ("Test", Some(ChunkTag::Dynamic)),
            (" error ", None),
            ("Error", Some(ChunkTag::Error)),
            (" done", None),
        ],
    )]
    #[case::multibyte(
        "🧡$__slumber$dy$💜$slumber__$",
        &[("🧡", None), ("💜", Some(ChunkTag::Dynamic))]
    )]
    // Test various cases where an incomplete tag is included. This could
    // be a bug in the generator, or it's really in the data. Either way, it's
    // treated as raw content
    #[case::prelude_only("$__slumber$", &[("$__slumber$", None)])]
    #[case::prelude_only_with_friends(
        "test $__slumber$ test", &[("test $__slumber$ test", None)],
    )]
    #[case::unknown_tag(
        "$__slumber$bad$Test$slumber__$",
        &[("$__slumber$bad$Test$slumber__$", None)],
    )]
    #[case::missing_terminator(
        "$__slumber$dy$Test$slu", &[("$__slumber$dy$Test$slu", None)],
    )]
    #[case::valid_and_invalid(
        "$__slumber$dy$Test$slumber__$ $__slumber$incomplete \
        $__slumber$dy$Test$slumber__$",
        &[
            ("Test", Some(ChunkTag::Dynamic)),
            (" $__slumber$incomplete ", None),
            ("Test", Some(ChunkTag::Dynamic)),
        ],
    )]
    fn test_parse_tagged_chunks(
        #[case] input: &str,
        #[case] expected: &[(&str, Option<ChunkTag>)],
    ) {
        let actual = parse_tagged_chunks(input);
        assert_eq!(actual, expected);
    }
}
