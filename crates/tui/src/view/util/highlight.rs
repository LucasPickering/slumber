//! Utilities for applying syntax highlighting to text.
//!
//! Warning: this thing is kinda fucked.

use anyhow::Context;
use itertools::Itertools;
use ratatui::{
    style::Style,
    text::{Line, Span, Text},
};
use slumber_core::http::content_type::ContentType;
use slumber_util::ResultTracedAnyhow;
use std::{
    borrow::Cow,
    cell::RefCell,
    collections::{HashMap, VecDeque},
};
use strum::{EnumIter, IntoEnumIterator};
use tree_sitter_highlight::{
    Highlight, HighlightConfiguration, HighlightEvent, Highlighter,
};

use crate::view::context::ViewContext;

thread_local! {
    /// Cache the highlighter and its configurations, because we only need one
    /// per thread. The view is single threaded, which means we only create one
    static HIGHLIGHTER: RefCell<(
        Highlighter,
        HashMap<ContentType, HighlightConfiguration>,
    )> = RefCell::default();
}

/// Apply syntax highlighting to some text. Syntax language will be determined
/// from the content type.
pub fn highlight(content_type: ContentType, mut text: Text<'_>) -> Text<'_> {
    HIGHLIGHTER.with_borrow_mut(|(highlighter, configs)| {
        let config = configs
            .entry(content_type)
            .or_insert_with(|| get_config(content_type));

        // Each line in the input corresponds to one line in the output, so we
        // can mutate each line inline
        for line in &mut text.lines {
            // Join the line into a single string so we can pass it to the
            // highlighter. Unfortunately it can't handle subline parsing, it
            // needs at least a line at a time
            let joined = join_line(line);
            let Ok(events) = highlighter
                .highlight(config, joined.as_bytes(), None, |_| None)
                .context("Syntax highlighting error")
                .traced()
            else {
                continue; // Leave the line untouched
            };

            let mut builder = LineBuilder::new(line);
            for event in events {
                match event.context("Syntax highlighting error").traced() {
                    Ok(HighlightEvent::Source { start, end }) => {
                        builder.push_span(&joined, start, end);
                    }
                    Ok(HighlightEvent::HighlightStart(index)) => {
                        let name = HighlightName::from_index(index);
                        builder.set_style(name.style());
                    }
                    Ok(HighlightEvent::HighlightEnd) => {
                        builder.reset_style();
                    }
                    // Not sure what would cause an error here, it doesn't seem
                    // like invalid syntax does it
                    Err(_) => {}
                }
            }

            *line = builder.build();
        }

        text
    })
}

/// Apply syntax highlighting if the content type is `Some`, otherwise just
/// return the given text
pub fn highlight_if(
    content_type: Option<ContentType>,
    text: Text<'_>,
) -> Text<'_> {
    if let Some(content_type) = content_type {
        highlight(content_type, text)
    } else {
        text
    }
}

/// Map [ContentType] to a syntax highlighting language
fn get_config(content_type: ContentType) -> HighlightConfiguration {
    let mut config = match content_type {
        ContentType::Json => HighlightConfiguration::new(
            tree_sitter_json::LANGUAGE.into(),
            "json",
            tree_sitter_json::HIGHLIGHTS_QUERY,
            "",
            "",
        )
        .expect("Error initializing JSON syntax highlighter"),
    };
    config.configure(
        HighlightName::iter()
            .map(HighlightName::to_str)
            .collect_vec()
            .as_slice(),
    );
    config
}

/// All highlight names that we support
///
/// <https://tree-sitter.github.io/tree-sitter/syntax-highlighting#highlights>
///
/// This enum should be the union of all highlight names in all supported langs:
/// - <https://github.com/tree-sitter/tree-sitter-json/blob/94f5c527b2965465956c2000ed6134dd24daf2a7/queries/highlights.scm>
#[derive(Copy, Clone, Debug, EnumIter)]
enum HighlightName {
    Comment,
    ConstantBuiltin,
    Escape,
    Number,
    String,
    StringSpecial,
}

impl HighlightName {
    /// Map to a string name, to pass to tree-sitter
    fn to_str(self) -> &'static str {
        match self {
            Self::Comment => "comment",
            Self::ConstantBuiltin => "constant.builtin",
            Self::Escape => "escape",
            Self::Number => "number",
            Self::String => "string",
            // This doesn't seem to work??
            Self::StringSpecial => "string.special",
        }
    }

    /// Tree-sitter passes highlights back as the index. This relies on a
    /// consistent iteration order of
    fn from_index(highlight: Highlight) -> Self {
        let index = highlight.0;
        Self::iter()
            .nth(index)
            .unwrap_or_else(|| panic!("Highlight index out of bounds: {index}"))
    }

    fn style(self) -> Style {
        let styles = ViewContext::styles().syntax_highlighting;
        match self {
            Self::Comment => styles.comment,
            Self::ConstantBuiltin => styles.builtin,
            Self::Escape => styles.escape,
            Self::Number => styles.number,
            Self::String => styles.string,
            Self::StringSpecial => styles.special,
        }
    }
}

/// Join all text in a line into a single string. For single-span lines (the
/// most common scenario by far), we'll just return the one span without a
/// clone.
fn join_line<'a>(line: &Line<'a>) -> Cow<'a, str> {
    if line.spans.is_empty() {
        Default::default()
    } else if line.spans.len() == 1 {
        // This is the hot path, most lines will just be one unstyled span. In
        // most scenarios we'll be getting borrowed content so the clone's cheap
        line.spans[0].content.clone()
    } else {
        // We have multiple spans, join them into a new string
        let mut text = String::with_capacity(
            line.spans.iter().map(|span| span.content.len()).sum(),
        );
        for span in &line.spans {
            text.push_str(&span.content);
        }
        text.into()
    }
}

/// Utility for merging styles on text. Use [Self::new] to initialize this
/// *before* highlighting, and it will remember which chunks of text had
/// preexisting styles. Use the setters to update state while processing
/// highlight events. These will be reapplied during highlighting, as the new
/// line is built up. After highlighting, call [Self::build] to get the new
/// line. The old styles will take precedence over the syntax highlighting.
///
/// This whole thing is required to retain template preview styling on top of
/// syntax highlighting.
struct LineBuilder<'a> {
    /// A set of **disjoint** style patches that we'll apply to the new line as
    /// it's being built. We need a deque because we'll pop off the front as
    /// we go
    patches: VecDeque<StylePatch>,
    /// New line being built
    line: Line<'a>,
    /// Style to be used for the *next* added span. This is updated
    /// imperatively as we loop over highlighter events.
    current_style: Style,
}

impl<'a> LineBuilder<'a> {
    /// Collect styles from a line to start a new builder
    fn new(line: &Line<'a>) -> Self {
        let mut patches = VecDeque::new();
        let mut len = 0;
        for span in &line.spans {
            if let Some(patch) = StylePatch::from_span(len, span) {
                patches.push_back(patch);
            }
            len += span.content.len();
        }

        Self {
            patches,
            line: Line::default(),
            current_style: Style::default(),
        }
    }

    /// Add a section of text to the new line. This will check if any cached
    /// styles apply to this section, and if so break it into multiple spans as
    /// needed to keep the old styling.
    #[expect(clippy::ptr_arg)]
    fn push_span(&mut self, text: &Cow<'a, str>, mut start: usize, end: usize) {
        // Keep a reference if we can. If the text is owned, we have to clone
        // because the owned value is going to get dropped after the build
        let mut content: Cow<'a, str> = match text {
            Cow::Borrowed(s) => s[start..end].into(),
            Cow::Owned(s) => s[start..end].to_owned().into(),
        };
        let style = self.current_style;

        while let Some(patch) = Self::next_patch(&mut self.patches, end) {
            // The first part of this chunk is not covered by the patch
            let (before, rest) = split_cow(content, patch.start - start);
            let (patched, after) = split_cow(rest, patch.len);
            let consumed = before.len() + patched.len();

            if !before.is_empty() {
                self.line.spans.push(Span {
                    content: before,
                    style,
                });
            }
            debug_assert!(!patched.is_empty(), "Patch should not be empty");
            self.line.spans.push(Span {
                content: patched,
                style: patch.style,
            });
            // Everything left over is for the next iteration
            content = after;
            start += consumed;
        }

        // Pull in whatever's left over. This is the hot path, because in most
        // cases we won't have any patches to apply
        if !content.is_empty() {
            self.line.spans.push(Span { content, style });
        }
    }

    /// Get the next patch in the sequence that applies before the given index.
    /// If the patch spans both sides of the index, split it and leave the
    /// second half in the queue
    fn next_patch(
        patches: &mut VecDeque<StylePatch>,
        before: usize,
    ) -> Option<StylePatch> {
        match patches.front() {
            Some(patch) if patch.start < before => {}
            _ => return None,
        }
        // Don't pop until we know we're going to use it
        let patch = patches.pop_front().unwrap();
        if before < patch.end() {
            let (left, right) = patch.split(before);
            patches.push_front(right);
            Some(left)
        } else {
            Some(patch)
        }
    }

    fn set_style(&mut self, style: Style) {
        self.current_style = style;
    }

    fn reset_style(&mut self) {
        self.current_style = Style::default();
    }

    /// Construct the line by applying pending style patches
    fn build(self) -> Line<'a> {
        debug_assert!(
            self.patches.is_empty(),
            "Patches remaining in queue: {:?}",
            &self.patches
        );
        self.line
    }
}

#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq))]
struct StylePatch {
    start: usize,
    len: usize,
    style: Style,
}

impl StylePatch {
    /// Create a new style patch for the given span, starting at the given
    /// index. If the span has default styling, return `None`.
    fn from_span(start: usize, span: &Span) -> Option<Self> {
        if span.style == Style::default() {
            None
        } else {
            Some(Self {
                start,
                len: span.content.len(),
                style: span.style,
            })
        }
    }

    fn end(&self) -> usize {
        self.start + self.len
    }

    /// Split this patch into two sections at a certain index
    fn split(self, at: usize) -> (Self, Self) {
        debug_assert!(
            self.start <= at && at < self.end(),
            "Split index {at} is not in [{}, {})",
            self.start,
            self.end()
        );
        let first_len = at - self.start;
        (
            Self {
                start: self.start,
                len: first_len,
                style: self.style,
            },
            Self {
                start: at,
                len: self.len - first_len,
                style: self.style,
            },
        )
    }
}

/// Split a cow into two substrings. If we have a borrowed string, return
/// subslices. If we have an owned string, we have to split into two owned
/// strings to prevent a self-reference.
fn split_cow(s: Cow<'_, str>, at: usize) -> (Cow<'_, str>, Cow<'_, str>) {
    match s {
        Cow::Borrowed(s) => {
            let (first, second) = s.split_at(at);
            (first.into(), second.into())
        }
        Cow::Owned(mut first) => {
            let second = first.split_off(at);
            (first.into(), second.into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::test_util::{TestHarness, harness};
    use pretty_assertions::assert_eq;
    use ratatui::style::Color;
    use rstest::rstest;

    /// Test that JSON is highlighted, by existing styling is retained
    #[rstest]
    fn test_highlight(_harness: TestHarness) {
        fn fg(color: Color) -> Style {
            Style::default().fg(color)
        }

        let text = vec![
            Line::from("{"),
            vec![
                "  \"string\": \"".into(),
                Span::styled("turkey", fg(Color::Blue)),
                "🦃".into(), // Throw some multi-byte chars in for fun
                Span::styled("day", fg(Color::Red)),
                "🦃\",".into(),
            ]
            .into(),
            "  \"number\": 3,".into(),
            // This whole thing should retain its style
            Span::styled("  \"bool\": false", fg(Color::Red)).into(),
            "}".into(),
        ]
        .into();
        let highlighted = highlight(ContentType::Json, text);
        let expected = vec![
            Line::from("{"),
            vec![
                "  ".into(),
                Span::styled("\"string\"", fg(Color::LightGreen)),
                ": ".into(),
                Span::styled("\"", fg(Color::LightGreen)),
                Span::styled("turkey", fg(Color::Blue)),
                Span::styled("🦃", fg(Color::LightGreen)),
                Span::styled("day", fg(Color::Red)),
                Span::styled("🦃\"", fg(Color::LightGreen)),
                ",".into(),
            ]
            .into(),
            vec![
                "  ".into(),
                Span::styled("\"number\"", fg(Color::LightGreen)),
                ": ".into(),
                Span::styled("3", fg(Color::Cyan)),
                ",".into(),
            ]
            .into(),
            // This whole line kept its styling, but it's broken up into spans
            // now for "technical" reasons
            vec![
                Span::styled("  ", fg(Color::Red)),
                Span::styled("\"bool\"", fg(Color::Red)),
                Span::styled(": ", fg(Color::Red)),
                Span::styled("false", fg(Color::Red)),
            ]
            .into(),
            "}".into(),
        ]
        .into();
        assert_eq!(highlighted, expected);
    }

    /// Test [StylePatch::split]
    #[test]
    fn test_patch_split() {
        let style = Style::default().fg(Color::Red);
        assert_eq!(
            StylePatch {
                start: 10,
                len: 4,
                style,
            }
            .split(13),
            (
                StylePatch {
                    start: 10,
                    len: 3,
                    style,
                },
                StylePatch {
                    start: 13,
                    len: 1,
                    style,
                }
            )
        );
    }
}
