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
        alt, cut_err, eof, not, preceded, repeat, repeat_till, terminated,
    },
    error::StrContext,
    token::{any, take_while},
    PResult, Parser,
};

/// Character used to escape other characters, converting their special meaning
/// into raw text
const ESCAPE: &str = "\\";
/// Marks the start of a template key
const KEY_OPEN: &str = "{{";
/// Marks the end of a template key
const KEY_CLOSE: &str = "}}";
/// Any sequence that can be escaped to strip its semantic meaning
const ESCAPABLE: [&str; 2] = [ESCAPE, KEY_OPEN];
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
    /// keyed templates.
    pub fn display(&self) -> Cow<'_, str> {
        let mut buf = String::new();

        // Re-stringify the template. For raw spans, we need to escape
        // special characters to get them to re-parse correctly later.
        for chunk in &self.chunks {
            match chunk {
                TemplateInputChunk::Raw(s) => {
                    let s = s.as_str();
                    let searcher = AhoCorasick::new(ESCAPABLE)
                        .expect("Invalid search string");
                    // Find each special sequence, and add a backslash
                    // before it
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

/// Match an escape sequence, e.g. `\\`` or `\{{`
fn escape_sequence<'a>(input: &mut &'a str) -> PResult<&'a str> {
    alt(ESCAPABLE.map(|text| (ESCAPE, text)))
        // Throw away the escape char
        .map(|(_, right)| right)
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
    use serde_test::{assert_de_tokens, assert_ser_tokens, Token};

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

    /// Test parsing success cases
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
    #[case::binary(r#"\xc3\x28"#, tmpl([raw(r#"\xc3\x28"#)]))]
    #[case::escape_key(
        r#"\{{escape}} {{field}} \{{escape}}"#,
        tmpl([raw("{{escape}} "), key_field("field"), raw(" {{escape}}")]),
    )]
    #[case::escape_incomplete_key(
        r#"escaped: \{{hello"#, tmpl([raw("escaped: {{hello")])
    )]
    #[case::escape_backslash(
        r#"unescaped: \\{{user_id}}"#,
        tmpl([raw(r#"unescaped: \"#), key_field("user_id")]),
    )]
    fn test_parse(#[case] template: &str, #[case] expected: Template) {
        let parsed: Template = template.parse().expect("Parsing failed");
        assert_eq!(parsed, expected);
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

    /// Test serialization and printing, which should escape raw chunks. Also
    /// test that it only allocates when necessary
    #[rstest]
    #[case::empty(tmpl([]), "", false)]
    #[case::raw(tmpl([raw("hello!")]), "hello!", false)]
    #[case::field(tmpl([key_field("user_id")]), "{{user_id}}",true)]
    #[case::env(tmpl([key_env("ENV1")]), "{{env.ENV1}}", true)]
    #[case::chain(tmpl([key_chain("chain1")]), "{{chains.chain1}}", true)]
    #[case::escape_key(
        tmpl([raw(r#"esc: {{user_id}}"#)]), r#"esc: \{{user_id}}"#, true
    )]
    #[case::escape_backslash(
        tmpl([raw(r#"esc: \"#), key_field("user_id")]),
        r#"esc: \\{{user_id}}"#,
        true // Escaping requires an allocation, since the text changes
    )]
    fn test_stringify(
        #[case] template: Template,
        #[case] expected: &'static str,
        #[case] should_allocate: bool,
    ) {
        let s = template.display();
        assert_eq!(s, expected);
        // Make sure we didn't make any unexpected clones
        if should_allocate {
            assert_matches!(s, Cow::Owned(_));
        } else {
            assert_matches!(s, Cow::Borrowed(_));
        }
        assert_ser_tokens(&template, &[Token::String(expected)]);
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
