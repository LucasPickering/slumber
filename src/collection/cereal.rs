//! Serialization/deserialization helpers for various types

use crate::{
    collection::{
        recipe_tree::RecipeNode, Chain, ChainId, Profile, ProfileId, RecipeId,
    },
    template::Template,
};
use serde::{
    de::{Error, Visitor},
    Deserialize, Deserializer,
};
use std::hash::Hash;

/// A type that has an `id` field. This is ripe for a derive macro, maybe a fun
/// project some day?
pub trait HasId {
    type Id: Clone + Eq + Hash;

    fn set_id(&mut self, id: Self::Id);
}

impl HasId for Profile {
    type Id = ProfileId;

    fn set_id(&mut self, id: Self::Id) {
        self.id = id;
    }
}

impl HasId for RecipeNode {
    type Id = RecipeId;

    fn set_id(&mut self, id: Self::Id) {
        match self {
            RecipeNode::Folder(folder) => folder.id = id,
            RecipeNode::Recipe(recipe) => recipe.id = id,
        }
    }
}

impl HasId for Chain {
    type Id = ChainId;

    fn set_id(&mut self, id: Self::Id) {
        self.id = id;
    }
}

/// Deserialize a map, and update each key so its `id` field matches its key in
/// the map. Useful if you need to access the ID when you only have a value
/// available, not the full entry.
pub fn deserialize_id_map<'de, Map, V, D>(
    deserializer: D,
) -> Result<Map, D::Error>
where
    Map: Deserialize<'de>,
    for<'m> &'m mut Map: IntoIterator<Item = (&'m V::Id, &'m mut V)>,
    D: Deserializer<'de>,
    V: Deserialize<'de> + HasId,
    V::Id: Deserialize<'de>,
{
    let mut map: Map = Map::deserialize(deserializer)?;
    // Update the ID on each value to match the key
    for (k, v) in &mut map {
        v.set_id(k.clone());
    }
    Ok(map)
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
                    Template::try_from(v.to_string()).map_err(E::custom)
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
            visit_primitive!(visit_str, &str);
        }

        deserializer.deserialize_any(TemplateVisitor)
    }
}

/// Serialize/deserialize a duration with unit shorthand. This does *not* handle
/// subsecond precision. Supported units are:
/// - s
/// - m
/// - h
/// - d
/// Examples: `30s`, `5m`, `12h`, `3d`
pub mod serde_duration {
    use regex::Regex;
    use serde::{de::Error, Deserialize, Deserializer, Serializer};
    use std::{sync::OnceLock, time::Duration};

    const UNIT_SECOND: &str = "s";
    const UNIT_MINUTE: &str = "m";
    const UNIT_HOUR: &str = "h";
    const UNIT_DAY: &str = "d";

    pub fn serialize<S>(
        duration: &Duration,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Always serialize as seconds, because it's easiest. Sub-second
        // precision is lost
        S::serialize_str(
            serializer,
            &format!("{}{}", duration.as_secs(), UNIT_SECOND),
        )
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        // unstable: use LazyLock https://github.com/rust-lang/rust/pull/121377
        static REGEX: OnceLock<Regex> = OnceLock::new();
        let s = String::deserialize(deserializer)?;
        let regex = REGEX.get_or_init(|| Regex::new("^(\\d+)(\\w+)$").unwrap());
        if let Some(captures) = regex.captures(&s) {
            let quantity: u64 = captures
                .get(1)
                .expect("No first group")
                .as_str()
                .parse()
                // Error should be impossible because the regex only allows ints
                .map_err(|_| D::Error::custom("Invalid int"))?;
            let unit = captures.get(2).expect("No second group").as_str();
            let seconds = match unit {
                UNIT_SECOND => quantity,
                UNIT_MINUTE => quantity * 60,
                UNIT_HOUR => quantity * 60 * 60,
                UNIT_DAY => quantity * 60 * 60 * 24,
                _ => {
                    return Err(D::Error::custom(format!(
                        "Unknown duration unit: {unit:?}; must be one of {:?}",
                        [UNIT_SECOND, UNIT_MINUTE, UNIT_HOUR, UNIT_DAY]
                    )))
                }
            };
            Ok(Duration::from_secs(seconds))
        } else {
            Err(D::Error::custom(
                "Invalid duration, must be \"<quantity><unit>\" (e.g. \"12d\")",
            ))
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use rstest::rstest;
        use serde::Serialize;
        use serde_test::{
            assert_de_tokens, assert_de_tokens_error, assert_ser_tokens, Token,
        };

        /// A wrapper that forces serde_test to use our custom
        /// serialize/deserialize functions
        #[derive(Debug, PartialEq, Serialize, Deserialize)]
        #[serde(transparent)]
        struct Wrap(#[serde(with = "super")] Duration);

        #[rstest]
        #[case::seconds_short(Duration::from_secs(3), "3s")]
        #[case::seconds_long(Duration::from_secs(3000), "3000s")]
        // Subsecond precision is lost
        #[case::seconds_subsecond_lost(Duration::from_millis(400), "0s")]
        #[case::seconds_subsecond_round_down(Duration::from_millis(1999), "1s")]
        fn test_serialize(
            #[case] duration: Duration,
            #[case] expected: &'static str,
        ) {
            assert_ser_tokens(&Wrap(duration), &[Token::String(expected)]);
        }

        #[rstest]
        #[case::seconds_zero("0s", Duration::from_secs(0))]
        #[case::seconds_short("1s", Duration::from_secs(1))]
        #[case::seconds_longer("100s", Duration::from_secs(100))]
        #[case::minutes("3m", Duration::from_secs(180))]
        #[case::hours("3h", Duration::from_secs(10800))]
        #[case::days("2d", Duration::from_secs(172800))]
        fn test_deserialize(
            #[case] s: &'static str,
            #[case] expected: Duration,
        ) {
            assert_de_tokens(&Wrap(expected), &[Token::Str(s)])
        }

        #[rstest]
        #[case::negative(
            "-1s",
            r#"Invalid duration, must be "<quantity><unit>" (e.g. "12d")"#
        )]
        #[case::whitespace(
            " 1s ",
            r#"Invalid duration, must be "<quantity><unit>" (e.g. "12d")"#
        )]
        #[case::decimal(
            "3.5s",
            r#"Invalid duration, must be "<quantity><unit>" (e.g. "12d")"#
        )]
        #[case::invalid_unit(
            "3hr",
            r#"Unknown duration unit: "hr"; must be one of ["s", "m", "h", "d"]"#
        )]
        fn test_deserialize_error(
            #[case] s: &'static str,
            #[case] error: &str,
        ) {
            assert_de_tokens_error::<Wrap>(&[Token::Str(s)], error)
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::template::Template;
    use rstest::rstest;
    use serde_test::{assert_de_tokens, Token};

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
    fn test_deserialize_template(#[case] token: Token, #[case] expected: &str) {
        assert_de_tokens(&Template::from(expected), &[token]);
    }
}
