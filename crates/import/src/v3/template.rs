//! v3 template implementation, which uses chains instead of functions

use crate::v3::models::ChainId;
use serde::{
    Deserialize, Deserializer,
    de::{self, Visitor},
};
use std::{str::FromStr, sync::Arc};
use winnow::{
    ModalResult, Parser,
    combinator::{
        alt, cut_err, eof, not, peek, preceded, repeat, repeat_till, terminated,
    },
    error::StrContext,
    token::{any, take_while},
};

/// Character used to escape key openings
const ESCAPE: &str = "_";
/// Marks the start of a template key
const KEY_OPEN: &str = "{{";
/// Marks the end of a template key
const KEY_CLOSE: &str = "}}";
// Export these so they can be used in TemplateKey's Display impl
pub const CHAIN_PREFIX: &str = "chains.";
pub const ENV_PREFIX: &str = "env.";

/// A parsed template, which can contain raw and/or templated content. The
/// string is parsed during creation to identify template keys, hence the
/// immutability.
///
/// The original string is *not* stored. To recover the source string, use the
/// `Display` implementation.
///
/// Invariants:
/// - Two templates with the same source string will have the same set of
///   chunks, and vice versa
/// - No two raw segments will ever be consecutive
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Template {
    /// Pre-parsed chunks of the template. For raw chunks we store the
    /// presentation text (which is not necessarily the source text, as escape
    /// sequences will be eliminated). For keys, just store the needed
    /// metadata.
    pub chunks: Vec<TemplateInputChunk>,
}

/// An identifier that can be used in a template key. A valid identifier is
/// any non-empty string that contains only alphanumeric characters, `-`, or
/// `_`.
///
/// Construct via [FromStr]
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq, Deserialize)]
#[serde(transparent)]
pub(super) struct Identifier(pub String);

/// A parsed template key. The variant of this determines how the key will be
/// resolved into a value.
///
/// This also serves as an enumeration of all possible value types. Once a key
/// is parsed, we know its value type and can dynamically dispatch for rendering
/// based on that.
///
/// The generic parameter defines *how* the key data is stored. Ideally we could
/// just store a `&str`, but that isn't possible when this is part of a
/// `Template`, because it would create a self-referential pointer. In that
/// case, we can store a `Span` which points back to its source in the template.
///
/// The `Display` impl here should return exactly what this was parsed from.
/// This is important for matching override keys during rendering.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum TemplateKey {
    /// A plain field, which can come from the profile or an override
    Field(Identifier),
    /// A value from a predefined chain of another recipe
    Chain(ChainId),
    /// A value pulled from the process environment
    Environment(Identifier),
}

/// Parse a template, extracting all template keys
impl FromStr for Template {
    type Err = String;

    fn from_str(template: &str) -> Result<Self, Self::Err> {
        let chunks = all_chunks
            .parse(template)
            .map_err(|error| error.to_string())?;
        Ok(Self { chunks })
    }
}

impl Identifier {
    /// Which characters are allowed in identifiers?
    fn is_char_allowed(c: char) -> bool {
        c.is_alphanumeric() || "-_".contains(c)
    }
}

impl FromStr for Identifier {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        identifier.parse(s).map_err(|error| error.to_string())
    }
}

/// A parsed piece of a template. After parsing, each chunk is either raw text
/// or a parsed key, ready to be rendered.
#[derive(Clone, Debug, PartialEq)]
pub enum TemplateInputChunk {
    /// Raw unprocessed text, i.e. something **outside** the `{{ }}`. This is
    /// stored in an `Arc` so we can share cheaply in each render, without
    /// having to clone text. This works because templates are immutable. Any
    /// non-empty string is a valid raw chunk. This text represents what the
    /// user wants to see, i.e. it does *not* including any escape chars.
    Raw(Arc<str>),
    Key(TemplateKey),
}

/// Parse a template into keys and raw text
///
/// Potential optimizations if parsing is slow:
/// - Use take_till or similar in raw string parsing
/// - <https://docs.rs/winnow/latest/winnow/_topic/performance/index.html>
fn all_chunks(input: &mut &str) -> ModalResult<Vec<TemplateInputChunk>> {
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
fn raw(input: &mut &str) -> ModalResult<Arc<str>> {
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
    .map(String::into)
    .context(StrContext::Label("raw text"))
    .parse_next(input)
}

/// Match an escape sequence `{_{`, `{__}`, etc. The trailing curly brace will
/// **not** be consumed.
fn escape_sequence<'a>(input: &mut &'a str) -> ModalResult<&'a str> {
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
fn key(input: &mut &str) -> ModalResult<TemplateKey> {
    preceded(
        KEY_OPEN,
        // Any error inside a template key is fatal, including an unclosed key
        cut_err(terminated(key_contents, KEY_CLOSE)),
    )
    .context(StrContext::Label("key"))
    .parse_next(input)
}

/// Parse the contents of a key (inside the `{{ }}`)
fn key_contents(input: &mut &str) -> ModalResult<TemplateKey> {
    alt((
        preceded(
            CHAIN_PREFIX,
            identifier.map(|id| TemplateKey::Chain(ChainId(id))),
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
fn identifier(input: &mut &str) -> ModalResult<Identifier> {
    take_while(1.., Identifier::is_char_allowed)
        .map(|id: &str| Identifier(id.to_owned()))
        .context(StrContext::Label("identifier"))
        .parse_next(input)
}

/// Custom deserializer for `Template`. This is useful for deserializing values
/// that are not strings, but should be treated as strings such as numbers,
/// booleans, and nulls.
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

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;
    use serde_test::{Token, assert_de_tokens};
    use slumber_util::assert_err;

    impl From<&'static str> for Identifier {
        fn from(value: &'static str) -> Self {
            Self(value.parse().unwrap())
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
        TemplateInputChunk::Key(TemplateKey::Chain(ChainId(chain_id.into())))
    }

    /// Parse and deserialize templates for various edge cases
    #[rstest]
    #[case::empty("", tmpl([]))]
    #[case::raw("raw", tmpl([raw("raw")]))]
    #[case::unopened_key("unopened}}", tmpl([raw("unopened}}")]))]
    #[case::field("{{field1}}", tmpl([key_field("field1")]))]
    #[case::field_number_id("{{1}}", tmpl([key_field("1")]))]
    #[case::chain("{{chains.chain1}}", tmpl([key_chain("chain1")]))]
    #[case::env("{{env.ENV}}", tmpl([key_env("ENV")]))]
    #[case::utf8(
        "intro\n{{user_id}} 💚💙💜 {{chains.chain}}\noutro\r\nmore outro",
        tmpl([
            raw("intro\n"),
            key_field("user_id"),
            raw(" 💚💙💜 "),
            key_chain("chain"),
            raw("\noutro\r\nmore outro"),
        ]),
    )]
    #[case::binary(r"\xc3\x28", tmpl([raw(r"\xc3\x28")]))]
    #[case::escape_incomplete_key("{_{hello {_{_{", tmpl([raw("{{hello {{{")]))]
    #[case::escape_key(
        // You should be able to put any number of underscores within a key,
        // and get n-1
        "{_{ {__{{user_id}} {___{{user_id}} {___{__{{user_id}}",
        tmpl([
            raw("{{ {_"),
            key_field("user_id"),
            raw(" {__"),
            key_field("user_id"),
            raw(" {__{_"),
            key_field("user_id"),
        ]),
    )]
    // `{_` should be treated literally when not followed by another {
    #[case::literal_underscores("{_a {_ _{", tmpl([raw("{_a {_ _{")]))]
    fn test_parse(#[case] input: &'static str, #[case] expected: Template) {
        let parsed: Template = input.parse().expect("Parsing failed");
        assert_eq!(parsed, expected, "incorrect parsed template");

        // Make sure serialization/deserialization impls work too
        assert_de_tokens(&expected, &[Token::Str(input)]);
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
    #[case::bonus_braces(r"\\{{{{field}}", "invalid identifier")]
    fn test_parse_error(#[case] template: &str, #[case] expected_error: &str) {
        assert_err!(template.parse::<Template>(), expected_error);
    }
}
