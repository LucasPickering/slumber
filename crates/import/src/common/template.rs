//! TODO

use crate::common::ChainId;
use derive_more::{Deref, Display};
use serde::{
    Deserialize, Deserializer, Serialize,
    de::{self, Visitor},
};
use std::{fmt::Debug, str::FromStr};
use thiserror::Error;
use winnow::{
    PResult, Parser,
    combinator::{
        alt, cut_err, eof, not, peek, preceded, repeat, repeat_till, terminated,
    },
    error::{ContextError, ParseError, StrContext},
    token::{any, take_while},
};

/// Character used to escape key openings
const ESCAPE: &str = "_";
/// Marks the start of a template key
const KEY_OPEN: &str = "{{";
/// Marks the end of a template key
const KEY_CLOSE: &str = "}}";
const CHAIN_PREFIX: &str = "chains.";
const ENV_PREFIX: &str = "env.";

/// A parsed template, which can contain raw and/or templated content. The
/// string is parsed during creation to identify template keys, hence the
/// immutability.
///
/// The original string is *not* stored. To recover the source string, use the
/// [Display] implementation.
///
/// Invariants:
/// - Two templates with the same source string will have the same set of
///   chunks, and vice versa
/// - No two raw segments will ever be consecutive
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Template {
    /// Pre-parsed chunks of the template. For raw chunks we store the
    /// presentation text (which is not necessarily the source text, as escape
    /// sequences will be eliminated). For keys, just store the needed
    /// metadata.
    pub chunks: Vec<TemplateInputChunk>,
}

impl Template {
    /// Create a new template from a raw string, without parsing it at all.
    /// Useful when importing from external formats where the string isn't
    /// expected to be a valid Slumber template
    pub fn raw(template: String) -> Template {
        let chunks = if template.is_empty() {
            vec![]
        } else {
            vec![TemplateInputChunk::Raw(template)]
        };
        Self { chunks }
    }

    /// Does this template contain any dynamic chunks?
    pub fn is_dynamic(&self) -> bool {
        // Raw segments can't be consecutive so if there's more than 1 chunk,
        // at least one of them must be dynamic
        !matches!(self.chunks.as_slice(), [] | [TemplateInputChunk::Raw(_)])
    }

    /// Create a template that renders a single field, equivalent to
    /// `{{<field>}}`
    pub fn from_field(field: Identifier) -> Self {
        Self {
            chunks: vec![TemplateInputChunk::Key(TemplateKey::Field(field))],
        }
    }

    /// Create a template that renders a single chain, equivalent to
    /// `{{chains.<id>}}`
    pub fn from_chain(id: ChainId) -> Self {
        Self {
            chunks: vec![TemplateInputChunk::Key(TemplateKey::Chain(id))],
        }
    }
}

/// Parse a template, extracting all template keys
impl FromStr for Template {
    type Err = TemplateParseError;

    fn from_str(template: &str) -> Result<Self, Self::Err> {
        let chunks = all_chunks.parse(template)?;
        Ok(Self { chunks })
    }
}

// Custom deserializer for `Template`. This is useful for deserializing values
// that are not strings, but should be treated as strings such as numbers,
// booleans, and nulls.
impl<'de> Deserialize<'de> for Template {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct TemplateVisitor;

        macro_rules! visit_primitive {
            ($func:ident, $type:ty) => {
                fn $func<E>(self, v: $type) -> Result<Self::Value, E>
                where
                    E: de::Error,
                {
                    self.visit_string(v.to_string())
                }
            };
        }

        impl Visitor<'_> for TemplateVisitor {
            type Value = Template;

            fn expecting(
                &self,
                formatter: &mut std::fmt::Formatter,
            ) -> std::fmt::Result {
                formatter.write_str("string, number, or boolean")
            }

            visit_primitive!(visit_bool, bool);
            visit_primitive!(visit_u64, u64);
            visit_primitive!(visit_i64, i64);
            visit_primitive!(visit_f64, f64);

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                v.parse().map_err(E::custom)
            }
        }

        deserializer.deserialize_any(TemplateVisitor)
    }
}

/// An identifier that can be used in a template key. A valid identifier is
/// any non-empty string that contains only alphanumeric characters, `-`, or
/// `_`.
///
/// Construct via [FromStr](std::str::FromStr)
///
/// TODO update comment, rename to not conflict with PS
#[derive(
    Clone,
    Debug,
    Deref,
    Default,
    Display,
    Eq,
    Hash,
    PartialEq,
    Serialize,
    Deserialize,
)]
#[serde(transparent)]
pub struct Identifier(String);

impl Identifier {
    /// Which characters are allowed in identifiers?
    fn is_char_allowed(c: char) -> bool {
        c.is_alphanumeric() || "-_".contains(c)
    }

    /// Generate an identifier from a string, replacing all invalid chars with
    /// a placeholder. Panic if the string is empty.
    pub fn escape(value: &str) -> Self {
        if value.is_empty() {
            panic!("Cannot create identifier from empty string");
        }
        Self(
            value
                .chars()
                .map(|c| if Self::is_char_allowed(c) { c } else { '_' })
                .collect(),
        )
    }
}

impl From<&'static str> for Identifier {
    /// BUild an identifier from a string literal. Panic if invalid
    fn from(value: &'static str) -> Self {
        Self(value.parse().unwrap())
    }
}

impl FromStr for Identifier {
    type Err = TemplateParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(identifier.parse(s)?)
    }
}

/// A parsed piece of a template. After parsing, each chunk is either raw text
/// or a parsed key, ready to be rendered.
#[derive(Clone, Debug, PartialEq)]
pub enum TemplateInputChunk {
    /// Raw unprocessed text, i.e. something **outside** the `{{ }}`. Any
    /// non-empty string is a valid raw chunk. This text represents what the
    /// user wants to see, i.e. it does *not* including any escape chars.
    Raw(String),
    Key(TemplateKey),
}

/// A parsed template key. The variant of this determines how the key will be
/// resolved into a value.
#[derive(Clone, Debug, PartialEq)]
pub enum TemplateKey {
    /// A plain field, which can come from the profile or an override
    Field(Identifier),
    /// A value from a predefined chain of another recipe
    Chain(ChainId),
    /// A value pulled from the process environment
    Environment(Identifier),
}

/// An error while parsing a template. This is derived from a winnow error
#[derive(Debug, Error)]
#[error("{0}")]
pub struct TemplateParseError(String);

/// Convert winnow's error type into ours. This stringifies the error so we can
/// dump the reference to the input
impl From<ParseError<&str, ContextError>> for TemplateParseError {
    fn from(error: ParseError<&str, ContextError>) -> Self {
        Self(error.to_string())
    }
}

/// Parse a template into keys and raw text
///
/// Potential optimizations if parsing is slow:
/// - Use take_till or similar in raw string parsing
/// - https://docs.rs/winnow/latest/winnow/_topic/performance/index.html
fn all_chunks(input: &mut &str) -> PResult<Vec<TemplateInputChunk>> {
    repeat_till(
        0..,
        alt((
            key.map(TemplateInputChunk::Key),
            raw.map(TemplateInputChunk::Raw),
        ))
        .context(StrContext::Label("template chunk")),
        eof,
    )
    .map(|(chunks, _)| chunks)
    .context(StrContext::Label("template"))
    .parse_next(input)
}

/// Parse raw text, until we hit a key or end of input
fn raw(input: &mut &str) -> PResult<String> {
    repeat(
        0..,
        alt((
            escape_sequence,
            // Match anything other than a key opening. This is inefficient
            // because it means we'll copy into the accumulating string one
            // char at a time. We could theoretically grab up to the next
            // escape seq or key here but I couldn't figure that out. Potential
            // optimization if perf is a problem
            (not(KEY_OPEN), any).take(),
        )),
    )
    .context(StrContext::Label("raw text"))
    .parse_next(input)
}

/// Match an escape sequence `{_{`, `{__}`, etc. The trailing curly brace will
/// **not** be consumed.
fn escape_sequence<'a>(input: &mut &'a str) -> PResult<&'a str> {
    terminated(
        // Parse {_+
        ("{", repeat::<_, _, (), _, _>(1.., ESCAPE))
            .take()
            // Drop the final underscore
            .map(|s: &str| &s[..s.len() - 1]),
        // Throw away the final _, don't consume the trailing {
        peek("{"),
    )
    .parse_next(input)
}

/// Parse a template key
fn key(input: &mut &str) -> PResult<TemplateKey> {
    preceded(
        KEY_OPEN,
        // Any error inside a template key is fatal, including an unclosed key
        cut_err(terminated(key_contents, KEY_CLOSE)),
    )
    .context(StrContext::Label("key"))
    .parse_next(input)
}

/// Parse the contents of a key (inside the `{{ }}`)
fn key_contents(input: &mut &str) -> PResult<TemplateKey> {
    alt((
        preceded(
            CHAIN_PREFIX,
            identifier.map(|id| TemplateKey::Chain(id.into())),
        )
        .context(StrContext::Label("chain")),
        preceded(ENV_PREFIX, identifier.map(TemplateKey::Environment))
            .context(StrContext::Label("environment")),
        identifier
            .map(TemplateKey::Field)
            .context(StrContext::Label("field")),
    ))
    .parse_next(input)
}

/// Parse a field name/chain ID/env variable etc, inside a key. See [Identifier]
/// for the definition of allowed syntax.
fn identifier(input: &mut &str) -> PResult<Identifier> {
    take_while(1.., Identifier::is_char_allowed)
        .map(|id: &str| Identifier(id.to_owned()))
        .context(StrContext::Label("identifier"))
        .parse_next(input)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;
    use serde_test::{Token, assert_de_tokens};
    use slumber_util::assert_err;

    /// Test deserialization, which has some additional logic on top of parsing
    #[rstest]
    // boolean
    #[case::bool_true(Token::Bool(true), "true")]
    #[case::bool_false(Token::Bool(false), "false")]
    // numeric
    #[case::u64(Token::U64(1000), "1000")]
    #[case::i64_negative(Token::I64(-1000), "-1000")]
    #[case::float_positive(Token::F64(10.1), "10.1")]
    #[case::float_negative(Token::F64(-10.1), "-10.1")]
    // string
    #[case::str(Token::Str("hello"), "hello")]
    #[case::str_null(Token::Str("null"), "null")]
    #[case::str_true(Token::Str("true"), "true")]
    #[case::str_false(Token::Str("false"), "false")]
    #[case::str_with_keys(Token::Str("{{user_id}}"), "{{user_id}}")]
    fn test_deserialize(#[case] token: Token, #[case] expected: &'static str) {
        assert_de_tokens(&Template::from(expected), &[token]);
    }

    /// Test parsing error cases. The error messages are not very descriptive
    /// so don't even bother looking for particular content
    #[rstest]
    #[case::unclosed_key("{{", "invalid identifier")]
    #[case::empty_key("{{}}", "invalid identifier")]
    #[case::invalid_key("{{.}}", "invalid identifier")]
    #[case::incomplete_dotted_key("{{bogus.}}", "invalid key")]
    #[case::invalid_dotted_key("{{bogus.one}}", "invalid key")]
    #[case::invalid_chain("{{chains.one.two}}", "invalid key")]
    #[case::invalid_env("{{env.one.two}}", "invalid key")]
    #[case::whitespace_key("{{ field }}", "invalid identifier")]
    // the first { is escaped, 2nd and 3rd make the key, 4th is a problem
    #[case::bonus_braces(r#"\\{{{{field}}"#, "invalid identifier")]
    fn test_parse_error(#[case] template: &str, #[case] expected_error: &str) {
        assert_err!(template.parse::<Template>(), expected_error);
    }

    /// Test that [Template::from_field] generates the correct template
    #[test]
    fn test_from_field() {
        let template = Template::from_field("field1".into());
        assert_eq!(&template.chunks, &[key_field("field1")]);
    }

    /// Test that [Template::from_chain] generates the correct template
    #[test]
    fn test_from_chain() {
        let template = Template::from_chain("chain1".into());
        assert_eq!(&template.chunks, &[key_chain("chain1")]);
    }

    /// Test [Template::raw]. This should parse+stringify back to the same thing
    #[rstest]
    #[case::empty("", tmpl([]))]
    #[case::key("{{hello}}", tmpl([raw("{{hello}}")]))]
    #[case::backslash(r#"\{{hello}}"#, tmpl([raw(r#"\{{hello}}"#)]))]
    fn test_raw(#[case] template: &str, #[case] expected: Template) {
        let escaped = Template::raw(template.into());
        assert_eq!(escaped, expected);
    }

    #[rstest]
    #[case::valid("valid-identifier_yeah", "valid-identifier_yeah")]
    #[case::invalid("not valid!", "not_valid_")]
    fn test_escape_identifier(#[case] input: &str, #[case] expected: &str) {
        let parsed = Identifier::escape(input);
        assert_eq!(parsed.as_str(), expected);
    }

    /// Escaping an empty identifier panics
    #[test]
    #[should_panic]
    fn test_escape_identifier_empty() {
        Identifier::escape("");
    }

    // Shortcuts for creating values from static strings. Since the string is
    // defined in code we're assuming it's valid.

    impl From<&'static str> for Template {
        fn from(value: &'static str) -> Self {
            value.parse().unwrap()
        }
    }

    impl From<&'static str> for ChainId {
        fn from(value: &'static str) -> Self {
            ChainId(value.into())
        }
    }

    /// Build a template out of string chunks. Useful when you want to avoid
    /// parsing behavior
    fn tmpl(chunks: impl IntoIterator<Item = TemplateInputChunk>) -> Template {
        Template {
            chunks: chunks.into_iter().collect(),
        }
    }

    /// Shorthand for creating a new raw chunk
    fn raw(value: &str) -> TemplateInputChunk {
        TemplateInputChunk::Raw(value.to_owned().into())
    }

    /// Shorthand for creating a field key chunk
    fn key_field(field: &'static str) -> TemplateInputChunk {
        TemplateInputChunk::Key(TemplateKey::Field(field.into()))
    }

    /// Shorthand for creating an env key chunk
    fn key_env(variable: &'static str) -> TemplateInputChunk {
        TemplateInputChunk::Key(TemplateKey::Environment(variable.into()))
    }

    /// Shorthand for creating a chain key chunk
    fn key_chain(chain_id: &'static str) -> TemplateInputChunk {
        TemplateInputChunk::Key(TemplateKey::Chain(chain_id.into()))
    }
}
