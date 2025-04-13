//! TODO

mod parse;

use crate::common::{ChainId, template::parse::TemplateInputChunk};
use derive_more::{Deref, Display};
use serde::{
    Deserialize, Deserializer, Serialize,
    de::{self, Visitor},
};
use std::fmt::Debug;

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
    chunks: Vec<TemplateInputChunk>,
}

impl Template {
    /// Create a new template from a raw string, without parsing it at all.
    /// Useful when importing from external formats where the string isn't
    /// expected to be a valid Slumber template
    pub fn raw(template: String) -> Template {
        let chunks = if template.is_empty() {
            vec![]
        } else {
            // This may seem too easy, but the hard part comes during
            // stringification, when we need to add backslashes to get the
            // string to parse correctly later
            vec![TemplateInputChunk::Raw(template.into())]
        };
        Self { chunks }
    }

    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
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
pub struct Identifier(
    #[cfg_attr(test, proptest(regex = "[a-zA-Z0-9-_]+"))] String,
);

/// A shortcut for creating identifiers from static strings. Since the string
/// is defined in code we're assuming it's valid.
impl From<&'static str> for Identifier {
    fn from(value: &'static str) -> Self {
        Self(value.parse().unwrap())
    }
}

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
#[cfg_attr(test, derive(proptest_derive::Arbitrary))]
pub enum TemplateKey {
    /// A plain field, which can come from the profile or an override
    Field(Identifier),
    /// A value from a predefined chain of another recipe
    Chain(ChainId),
    /// A value pulled from the process environment
    Environment(Identifier),
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;
    use serde_test::{Token, assert_de_tokens};

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
}
