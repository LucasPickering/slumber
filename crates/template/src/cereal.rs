use crate::{Template, TemplateError, Value};
use serde::{
    Deserialize, Deserializer, Serialize,
    de::{
        self, Error, IntoDeserializer, Visitor,
        value::{MapDeserializer, SeqDeserializer},
    },
};
use std::fmt::Display;

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
                E: Error,
            {
                v.parse().map_err(E::custom)
            }
        }

        deserializer.deserialize_any(TemplateVisitor)
    }
}

/// Deserialize from a template [Value]. Used for deserializing values into
/// function arguments
pub struct ValueDeserializer(Value);

impl IntoDeserializer<'_, TemplateError> for Value {
    type Deserializer = ValueDeserializer;

    fn into_deserializer(self) -> ValueDeserializer {
        ValueDeserializer(self)
    }
}

impl<'de> serde::Deserializer<'de> for ValueDeserializer {
    type Error = TemplateError;

    fn deserialize_any<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: de::Visitor<'de>,
    {
        match self.0 {
            Value::Null => visitor.visit_none(),
            Value::Bool(b) => visitor.visit_bool(b),
            Value::Int(i) => visitor.visit_i64(i),
            Value::Float(f) => visitor.visit_f64(f),
            Value::String(string) => visitor.visit_string(string),
            Value::Bytes(buffer) => visitor.visit_byte_buf(buffer.into()),
            Value::Array(array) => {
                visitor.visit_seq(&mut SeqDeserializer::new(array.into_iter()))
            }
            Value::Object(object) => {
                visitor.visit_map(&mut MapDeserializer::new(object.into_iter()))
            }
        }
    }

    serde::forward_to_deserialize_any! {
        unit bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str
        string bytes byte_buf identifier ignored_any unit_struct struct map seq
        tuple tuple_struct enum newtype_struct option
    }
}

impl de::Error for TemplateError {
    fn custom<T>(msg: T) -> Self
    where
        T: Display,
    {
        TemplateError::Other(msg.to_string().into())
    }
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
    fn test_deserialize_template(#[case] token: Token, #[case] expected: &str) {
        assert_de_tokens(&Template::from(expected), &[token]);
    }
}
