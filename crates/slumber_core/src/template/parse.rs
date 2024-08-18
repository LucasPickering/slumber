//! Parsing and stringification for templates

use crate::{
    collection::ChainId,
    template::{error::TemplateParseError, Identifier, Template, TemplateKey},
};
use aho_corasick::AhoCorasick;
use serde::{
    de::{Error, Visitor},
    Deserialize, Deserializer, Serialize,
};
use std::{borrow::Cow, fmt::Write, str::FromStr, sync::Arc};
use winnow::{
    combinator::{
        alt, cut_err, delimited, eof, not, peek, preceded, repeat, repeat_till,
        terminated,
    },
    error::StrContext,
    token::{any, take_while},
    PResult, Parser,
};

/// Character used to escape keys
const ESCAPE: &str = r#"\"#;
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
        let mut buf = String::new();

        // Re-stringify the template. For raw spans, we need to escape
        // the key open sequence so it doesn't parse back as a key
        for chunk in &self.chunks {
            match chunk {
                TemplateInputChunk::Raw(s) => {
                    let s = s.as_str();
                    let searcher = AhoCorasick::new([KEY_OPEN])
                        .expect("Invalid search string");
                    // Find each escape sequence, and add a backslash before it
                    let mut i = 0;
                    for m in searcher.find_iter(s) {
                        // Write everything before the special char, then
                        // escape. The escaped sequence will be written on the
                        // next iter
                        buf.push_str(&s[i..m.start()]);
                        buf.push_str(ESCAPE);
                        i = m.start();
                    }

                    // If we have just a single raw chunk, and it doesn't
                    // contain any escape sequences, we can return a reference
                    // to it and avoid any allocation or copying
                    if self.chunks.len() == 1 && buf.is_empty() {
                        return s.into();
                    }

                    // Fencepost: segment between last match and end
                    buf.push_str(&s[i..]);
                }
                TemplateInputChunk::Key(key) => {
                    // If the previous chunk ends with a \, then we need to
                    // escape THAT backslash so it doesn't escape us. Fight
                    // backslashes with backslashes.
                    if buf.ends_with(ESCAPE) {
                        buf.push_str(ESCAPE);
                    }
                    write!(&mut buf, "{KEY_OPEN}{key}{KEY_CLOSE}").unwrap();
                }
            }
        }

        // This doesn't seem important because an empty String doesn't allocate
        // either, but consumers of Cow can optimize better with a borrowed str
        if buf.is_empty() {
            return "".into();
        }

        buf.into()
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

impl Serialize for Template {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.display().serialize(serializer)
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
                    E: Error,
                {
                    self.visit_string(v.to_string())
                }
            };
        }

        impl<'de> Visitor<'de> for TemplateVisitor {
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
                E: Error,
            {
                v.parse().map_err(E::custom)
            }
        }

        deserializer.deserialize_any(TemplateVisitor)
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
    /// Raw unprocessed text, i.e. something **outside** the `{{ }}`. This is
    /// stored in an `Arc` so we can share cheaply in each render, without
    /// having to clone text. This works because templates are immutable.
    Raw(Arc<String>),
    Key(TemplateKey),
}

/// Parse a template into keys and raw text
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
fn raw(input: &mut &str) -> PResult<Arc<String>> {
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

/// Match an escape sequence: `\{{` or `\\{{`. Other sequences involving
/// backslashes will *not* be consumed here. They should be treated as normal
/// text. This is to prevent modifying common escape sequences from whatever
/// syntax may be within the template e.g. escaped quotes.
fn escape_sequence<'a>(input: &mut &'a str) -> PResult<&'a str> {
    alt((
        // \{{ -> {{
        preceded(ESCAPE, KEY_OPEN),
        // \\{{ -> \ (key open remains, to be parsed later)
        delimited(ESCAPE, ESCAPE, peek(KEY_OPEN)),
    ))
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
    use crate::{assert_err, assert_matches};
    use rstest::rstest;
    use serde_test::{assert_de_tokens, assert_tokens, Token};

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
    #[case::binary(r#"\xc3\x28"#, tmpl([raw(r#"\xc3\x28"#)]), false)]
    #[case::escape_key(
        r#"\{{escape}} {{field}} \{{escape}}"#,
        tmpl([raw("{{escape}} "), key_field("field"), raw(" {{escape}}")]),
        true,
    )]
    #[case::escape_incomplete_key(
        r#"escaped: \{{hello"#, tmpl([raw("escaped: {{hello")]), true
    )]
    #[case::escape_backslash(
        // You should be able to put any number of backslashes before a key,
        // and only one gets subtracted out (to escape the key)
        r#"\\{{user_id}} \\\{{user_id}} \\\\{{user_id}}"#,
        tmpl([
            raw(r#"\"#),
            key_field("user_id"),
            raw(r#" \\"#),
            key_field("user_id"),
            raw(r#" \\\"#),
            key_field("user_id"),
        ]),
        true,
    )]
    #[case::unescaped_backslashes(
        // Standalone backslashes (not preceding a key) are left alone
        r#""{\"escaped\": \"quotes\""#,
        tmpl([raw(r#""{\"escaped\": \"quotes\""#)]),
        false,
    )]
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
    #[case::backslash(r#"\{{hello}}"#, tmpl([raw(r#"\{{hello}}"#)]))]
    fn test_raw(#[case] template: &str, #[case] expected: Template) {
        let escaped = Template::raw(template.into());
        assert_eq!(escaped, expected);
    }

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
    fn test_deserialize(#[case] token: Token, #[case] expected: &str) {
        assert_de_tokens(&Template::from(expected), &[token]);
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
}
