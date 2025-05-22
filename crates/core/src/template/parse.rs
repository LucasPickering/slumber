//! Parsing and stringification for templates

use crate::{
    collection::ChainId,
    template::{Identifier, Template, TemplateKey, error::TemplateParseError},
};
#[cfg(test)]
use proptest::strategy::Strategy;
use regex::Regex;
use std::{
    borrow::Cow,
    fmt::Write,
    str::FromStr,
    sync::{Arc, LazyLock},
};
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

impl Template {
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

    /// Convert the template to a string. This will only allocate for escaped or
    /// keyed templates. This is guaranteed to return the exact string that was
    /// parsed to create the template, and therefore will parse back to the same
    /// template. If it doesn't, that's a bug.
    pub fn display(&self) -> Cow<'_, str> {
        let mut buf = Cow::Borrowed("");

        // Re-stringify the template
        for chunk in &self.chunks {
            match chunk {
                TemplateInputChunk::Raw(s) => {
                    // Add underscores between { to escape them. Any sequence
                    // of {_* followed by another { needs to be escaped. Regex
                    // matches have to be non-overlapping so we can't  just use
                    // {_*{, because that wouldn't catch cases like {_{_{. So
                    // we have to do our own lookahead.
                    //
                    // Keep in mind that escape sequences are going to be an
                    // extreme rarity, so we need to optimize for the case where
                    // there are none and only allocate when necessary.
                    static REGEX: LazyLock<Regex> =
                        LazyLock::new(|| Regex::new(r"\{_*").unwrap());
                    // Track how far into s we've copied, so we can do as few
                    // copies as possible
                    let mut last_copied = 0;
                    for m in REGEX.find_iter(s) {
                        let rest = &s[m.end()..];
                        // Don't allocate until we know this needs an escape
                        // sequence
                        if rest.starts_with('{') {
                            let buf = buf.to_mut();
                            buf.push_str(&s[last_copied..m.end()]);
                            buf.push('_');
                            last_copied = m.end();
                        }
                    }

                    // If this is the first chunk and there were no regex
                    // matches, don't allocate yet
                    if buf.is_empty() {
                        buf = Cow::Borrowed(s);
                    } else {
                        // Fencepost: get everything from the last escape
                        // sequence to the end
                        buf.to_mut().push_str(&s[last_copied..]);
                    }
                }
                TemplateInputChunk::Key(key) => {
                    // If the previous chunk ends with a potential escape
                    // sequence, add an underscore to escape the upcoming key
                    static REGEX: LazyLock<Regex> =
                        LazyLock::new(|| Regex::new(r"\{_*$").unwrap());
                    if REGEX.is_match(&buf) {
                        buf.to_mut().push_str(ESCAPE);
                    }

                    write!(buf.to_mut(), "{KEY_OPEN}{key}{KEY_CLOSE}").unwrap();
                }
            }
        }

        buf
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

impl Identifier {
    /// Which characters are allowed in identifiers?
    fn is_char_allowed(c: char) -> bool {
        c.is_alphanumeric() || "-_".contains(c)
    }

    /// Generate an identifier from a string, replacing all invalid chars with
    /// a placeholder. Panic if the string is empty.
    pub fn escape(value: &str) -> Self {
        assert!(
            !value.is_empty(),
            "Cannot create identifier from empty string"
        );
        Self(
            value
                .chars()
                .map(|c| if Self::is_char_allowed(c) { c } else { '_' })
                .collect(),
        )
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
#[cfg_attr(test, derive(proptest_derive::Arbitrary))]
pub enum TemplateInputChunk {
    /// Raw unprocessed text, i.e. something **outside** the `{{ }}`. This is
    /// stored in an `Arc` so we can share cheaply in each render, without
    /// having to clone text. This works because templates are immutable. Any
    /// non-empty string is a valid raw chunk. This text represents what the
    /// user wants to see, i.e. it does *not* including any escape chars.
    Raw(
        #[cfg_attr(test, proptest(strategy = "\".+\".prop_map(Arc::new)"))]
        Arc<String>,
    ),
    Key(TemplateKey),
}

/// Parse a template into keys and raw text
///
/// Potential optimizations if parsing is slow:
/// - Use take_till or similar in raw string parsing
/// - https://docs.rs/winnow/latest/winnow/_topic/performance/index.html
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
fn raw(input: &mut &str) -> ModalResult<Arc<String>> {
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
    .map(Arc::new)
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
fn identifier(input: &mut &str) -> ModalResult<Identifier> {
    take_while(1.., Identifier::is_char_allowed)
        .map(|id: &str| Identifier(id.to_owned()))
        .context(StrContext::Label("identifier"))
        .parse_next(input)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::proptest;
    use rstest::rstest;
    use serde_test::{Token, assert_tokens};
    use slumber_util::{assert_err, assert_matches};

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

    /// Test round tripping between raw strings and templates. Parse, display,
    /// make sure we get the same thing back. Also check if stringification
    /// allocated, to make sure optimizations work as intended.
    #[rstest]
    #[case::empty("", tmpl([]), false)]
    #[case::raw("raw", tmpl([raw("raw")]), false)]
    #[case::unopened_key("unopened}}", tmpl([raw("unopened}}")]), false)]
    #[case::field("{{field1}}", tmpl([key_field("field1")]), true)]
    #[case::field_number_id("{{1}}", tmpl([key_field("1")]), true)]
    #[case::chain("{{chains.chain1}}", tmpl([key_chain("chain1")]), true)]
    #[case::env("{{env.ENV}}", tmpl([key_env("ENV")]), true)]
    #[case::utf8(
        "intro\n{{user_id}} 💚💙💜 {{chains.chain}}\noutro\r\nmore outro",
        tmpl([
            raw("intro\n"),
            key_field("user_id"),
            raw(" 💚💙💜 "),
            key_chain("chain"),
            raw("\noutro\r\nmore outro"),
        ]),
        true
    )]
    #[case::binary(r"\xc3\x28", tmpl([raw(r"\xc3\x28")]), false)]
    #[case::escape_incomplete_key(
        "{_{hello {_{_{", tmpl([raw("{{hello {{{")]), true
    )]
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
        true,
    )]
    // `{_` should be treated literally when not followed by another {
    #[case::literal_underscores("{_a {_ _{", tmpl([raw("{_a {_ _{")]), false)]
    fn test_parse_display(
        #[case] input: &'static str,
        #[case] expected: Template,
        #[case] display_should_allocate: bool,
    ) {
        let parsed: Template = input.parse().expect("Parsing failed");
        assert_eq!(parsed, expected, "incorrect parsed template");
        let stringified = parsed.display();
        assert_eq!(stringified, input, "incorrect stringified template");
        // Make sure we didn't make any unexpected clones
        if display_should_allocate {
            assert_matches!(stringified, Cow::Owned(_));
        } else {
            assert_matches!(stringified, Cow::Borrowed(_));
        }

        // Make sure serialization/deserialization impls work too
        assert_tokens(&expected, &[Token::Str(input)]);
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

    /// Test that [Template::from_field] generates the correct template
    #[test]
    fn test_from_field() {
        let template = Template::from_field("field1".into());
        assert_eq!(template.display(), "{{field1}}");
        assert_eq!(&template.chunks, &[key_field("field1")]);
    }

    /// Test that [Template::from_chain] generates the correct template
    #[test]
    fn test_from_chain() {
        let template = Template::from_chain("chain1".into());
        assert_eq!(template.display(), "{{chains.chain1}}");
        assert_eq!(&template.chunks, &[key_chain("chain1")]);
    }

    /// Test [Template::raw]. This should parse+stringify back to the same thing
    #[rstest]
    #[case::empty("", tmpl([]))]
    #[case::key("{{hello}}", tmpl([raw("{{hello}}")]))]
    #[case::backslash(r"\{{hello}}", tmpl([raw(r"\{{hello}}")]))]
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
    #[should_panic(expected = "Cannot create identifier from empty string")]
    fn test_escape_identifier_empty() {
        Identifier::escape("");
    }

    proptest! {
        #[test]
        fn test_round_trip_prop(template: Template) {
            let s = template.display();
            let parsed = s.parse::<Template>()
                .expect("Error parsing stringified template");
            assert_eq!(parsed, template);
        }
    }
}
