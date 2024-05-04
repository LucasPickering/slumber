//! Template string parser

use crate::template::{error::TemplateParseError, Template, TemplateKey};
use nom::{
    branch::alt,
    bytes::complete::{tag, take_while1},
    combinator::{all_consuming, cut},
    error::{context, ErrorKind, ParseError, VerboseError},
    multi::many0,
    sequence::{preceded, terminated},
    FindSubstring, Finish, IResult, InputLength, InputTake, Offset, Parser,
};

const KEY_OPEN: &str = "{{";
const KEY_CLOSE: &str = "}}";
// Export these so they can be used in TemplateKey's Display impl
pub const CHAIN_PREFIX: &str = "chains.";
pub const ENV_PREFIX: &str = "env.";

type ParseResult<'a, T> = IResult<&'a str, T, VerboseError<&'a str>>;

impl Template {
    /// Parse a template, extracting all template keys
    pub fn parse(template: String) -> Result<Self, TemplateParseError> {
        // Parse everything as string slices
        let (_, chunks) = all_chunks(&template)
            .finish()
            .map_err(|error| TemplateParseError::new(&template, error))?;

        // Map all the string slices to spans to avoid self-references. It would
        // be better to track the spans as we go, but that's a bit harder
        let mapper = |s: &str| Span::new(template.offset(s), s.len());
        let chunks =
            chunks.into_iter().map(|chunk| chunk.map(mapper)).collect();

        Ok(Self { template, chunks })
    }
}

/// A parsed piece of a template. After parsing, each chunk is either raw text
/// or a parsed key, ready to be rendered.
#[derive(Copy, Clone, Debug)]
#[cfg_attr(test, derive(PartialEq))]
pub enum TemplateInputChunk<T> {
    Raw(T),
    Key(TemplateKey<T>),
}

impl<T> TemplateInputChunk<T> {
    /// Map the internal data using the given function. Useful for mapping
    /// string slices to spans and vice versa.
    fn map<U>(self, f: impl Fn(T) -> U) -> TemplateInputChunk<U> {
        match self {
            Self::Raw(value) => TemplateInputChunk::Raw(f(value)),
            Self::Key(key) => TemplateInputChunk::Key(key.map(f)),
        }
    }
}

/// Indexes defining a substring of text within some string. This is a useful
/// alternative to string slices when avoiding self-referential structs.
#[derive(Copy, Clone, Debug)]
#[cfg_attr(test, derive(PartialEq))]
pub struct Span {
    start: usize,
    /// Store length instead of end so it can never be invalid
    len: usize,
}

impl Span {
    pub(super) fn new(start: usize, len: usize) -> Self {
        Self { start, len }
    }

    /// Starting index of the span, inclusive
    pub fn start(&self) -> usize {
        self.start
    }

    /// Ending index of the span, exclusive
    pub fn end(&self) -> usize {
        self.start + self.len
    }
}

/// Parse a template into keys and raw text
fn all_chunks(input: &str) -> ParseResult<Vec<TemplateInputChunk<&str>>> {
    all_consuming(many0(alt((
        key.map(TemplateInputChunk::Key),
        raw.map(TemplateInputChunk::Raw),
    ))))(input)
}

/// Parse raw text, until we hit a key or end of input
fn raw(input: &str) -> ParseResult<&str> {
    context("raw", take_until_or_eof(KEY_OPEN))(input)
}

/// Parse a template key
fn key(input: &str) -> ParseResult<TemplateKey<&str>> {
    context(
        "key",
        preceded(
            tag(KEY_OPEN),
            // Any error inside a template key is fatal, including an unclosed
            // key
            cut(terminated(key_contents, tag(KEY_CLOSE))),
        ),
    )(input)
}

/// Parse the contents of a key (inside the `{{ }}`)
fn key_contents(input: &str) -> ParseResult<TemplateKey<&str>> {
    alt((
        context(
            "chain",
            preceded(tag(CHAIN_PREFIX), identifier).map(TemplateKey::Chain),
        ),
        context(
            "environment",
            preceded(tag(ENV_PREFIX), identifier).map(TemplateKey::Environment),
        ),
        context("field", identifier.map(TemplateKey::Field)),
    ))(input)
}

/// Parse a field name/chain ID/env variable etc, inside a key
fn identifier(input: &str) -> ParseResult<&str> {
    context(
        "identifier",
        take_while1(|c: char| c.is_alphanumeric() || "-_".contains(c)),
    )(input)
}

/// A copy pasta of nom's `take_until` that will take up to the end of a string
/// if the terminator never appears, instead of erroring out. I couldn't
/// figure out how to do this with other combinators, so here we go
fn take_until_or_eof<'a>(
    tag: &'a str,
) -> impl Fn(&'a str) -> ParseResult<&'a str> {
    |i| match i.find_substring(tag) {
        Some(index) => Ok(i.take_split(index)),
        None if i.input_len() > 0 => Ok(i.take_split(i.input_len())),
        None => Err(nom::Err::Error(VerboseError::from_error_kind(
            i,
            ErrorKind::TakeUntil,
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::*;
    use itertools::Itertools;
    use rstest::rstest;

    /// Test parsing success cases
    #[rstest]
    #[case::empty("", vec![])]
    #[case::raw("raw", vec![TemplateInputChunk::Raw("raw")])]
    #[case::unopened_key("unopened}}", vec![TemplateInputChunk::Raw("unopened}}")])]
    #[case::field(
        "{{field1}}",
        vec![TemplateInputChunk::Key(TemplateKey::Field("field1"))]
    )]
    #[case::field_number_id("{{1}}", vec![TemplateInputChunk::Key(TemplateKey::Field("1"))])]
    #[case::chain(
        "{{chains.chain1}}",
        vec![TemplateInputChunk::Key(TemplateKey::Chain("chain1"))]
    )]
    #[case::env(
        "{{env.ENV}}",
        vec![TemplateInputChunk::Key(TemplateKey::Environment("ENV"))]
    )]
    #[case::utf8(
        "intro\n{{user_id}} 💚💙💜 {{chains.chain}}\noutro\r\nmore outro",
        vec![
            TemplateInputChunk::Raw("intro\n"),
            TemplateInputChunk::Key(TemplateKey::Field("user_id")),
            TemplateInputChunk::Raw(" 💚💙💜 "),
            TemplateInputChunk::Key(TemplateKey::Chain("chain")),
            TemplateInputChunk::Raw("\noutro\r\nmore outro"),
        ]
    )]
    fn test_parse(
        #[case] template: &str,
        #[case] expected_chunks: Vec<TemplateInputChunk<&str>>,
    ) {
        let parsed =
            Template::parse(template.to_owned()).expect("Parsing failed");
        // Map from spans to strings to make test creation easier
        let chunks = parsed
            .chunks
            .iter()
            .map(|chunk| chunk.map(|span| parsed.substring(span)))
            .collect_vec();
        assert_eq!(chunks, expected_chunks);
    }

    /// Test parsing error cases. The error messages are not very descriptive
    /// so don't even bother looking for particular content
    #[rstest]
    #[case::unclosed_key("{{")]
    #[case::empty_key("{{}}")]
    #[case::invalid_key("{{.}}")]
    #[case::incomplete_dotted_key("{{bogus.}}")]
    #[case::invalid_dotted_key("{{bogus.one}}")]
    #[case::invalid_chain("{{chains.one.two}}")]
    #[case::invalid_env("{{env.one.two}}")]
    #[case::whitespace("{{ field }}")]
    fn test_parse_error(#[case] template: &str) {
        assert_err!(Template::parse(template.into()), "at line 1");
    }
}
