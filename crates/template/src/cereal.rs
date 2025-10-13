//! Deserialization for the config. This uses saphyr-based deserialization. It
//! would be great to use serde for this, but saphyr doesn't have serde support
//! yet. Once saphyr supports serde, we can delete all of this and delete any
//! `DeserializeYaml` implementations.

use crate::{Template, Value, ValueError};
use indexmap::IndexMap;
use saphyr::{Scalar, YamlData};
use serde::{
    Serialize,
    de::{
        self, IntoDeserializer, Unexpected, Visitor,
        value::{MapDeserializer, SeqDeserializer},
    },
};
use slumber_util::yaml::{
    DeserializeYaml, Expected, LocatedError, SourceMap, SourcedYaml,
};

impl Serialize for Template {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.display().serialize(serializer)
    }
}

/// Deserialize templates via saphyr
impl DeserializeYaml for Template {
    fn expected() -> Expected {
        Expected::OneOf(&[
            &Expected::String,
            &Expected::Boolean,
            &Expected::Number,
            // We accept `null` too, but it's not a helpful suggestion
        ])
    }

    fn deserialize(
        yaml: SourcedYaml,
        _source_map: &SourceMap,
    ) -> Result<Self, LocatedError<slumber_util::yaml::YamlErrorKind>> {
        if let YamlData::Value(scalar) = yaml.data {
            // Accept any scalar for a template. We'll treat everything as the
            // equivalent string representation
            match scalar {
                Scalar::Null => "null".parse(),
                Scalar::Boolean(b) => b.to_string().parse(),
                Scalar::Integer(i) => i.to_string().parse(),
                Scalar::FloatingPoint(f) => f.to_string().parse(),
                Scalar::String(s) => s.parse(),
            }
            .map_err(|error| LocatedError::other(error, yaml.location))
        } else {
            Err(LocatedError::unexpected(Expected::String, yaml))
        }
    }
}

#[cfg(feature = "schema")]
impl schemars::JsonSchema for Template {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Template".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": ["string", "boolean", "number"],
        })
    }
}

/// Deserialize from a template [Value]. Used for deserializing values into
/// function arguments
pub struct ValueDeserializer<'de>(&'de Value);

impl<'de> IntoDeserializer<'de, ValueError> for &'de Value {
    type Deserializer = ValueDeserializer<'de>;

    fn into_deserializer(self) -> Self::Deserializer {
        ValueDeserializer(self)
    }
}

// Deserialize an object value
impl<'de> IntoDeserializer<'de, ValueError> for &'de IndexMap<String, Value> {
    type Deserializer = MapDeserializer<
        'de,
        Box<dyn 'de + Iterator<Item = (&'de str, &'de Value)>>,
        ValueError,
    >;

    fn into_deserializer(self) -> Self::Deserializer {
        MapDeserializer::new(Box::new(
            self.iter().map(|(k, v)| (k.as_str(), v)),
        ))
    }
}

impl<'de> serde::Deserializer<'de> for ValueDeserializer<'de> {
    type Error = ValueError;

    fn deserialize_any<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: de::Visitor<'de>,
    {
        match self.0 {
            Value::Null => visitor.visit_none(),
            Value::Boolean(b) => visitor.visit_bool(*b),
            Value::Integer(i) => visitor.visit_i64(*i),
            Value::Float(f) => visitor.visit_f64(*f),
            Value::String(s) => visitor.visit_str(s),
            Value::Bytes(buffer) => {
                // In most cases where bytes are returned, the user actually
                // wants a string (e.g. in a JSON value). If we can convert to
                // a string, deserialize as that.
                //
                // If the user actually wants bytes no matter what, the
                // Deserialize impl should call deserialize_bytes or
                // deserialize_byte_buf
                match std::str::from_utf8(buffer) {
                    Ok(s) => visitor.visit_str(s),
                    Err(_) => visitor.visit_bytes(buffer),
                }
            }
            Value::Array(array) => {
                visitor.visit_seq(&mut SeqDeserializer::new(array.iter()))
            }
            Value::Object(object) => {
                visitor.visit_map(&mut object.into_deserializer())
            }
        }
    }
    fn deserialize_bytes<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let unexpected = match self.0 {
            Value::Bytes(buffer) => return visitor.visit_bytes(buffer),
            Value::String(s) => return visitor.visit_bytes(s.as_bytes()),
            Value::Null => Unexpected::Unit,
            Value::Boolean(b) => Unexpected::Bool(*b),
            Value::Integer(i) => Unexpected::Signed(*i),
            Value::Float(f) => Unexpected::Float(*f),
            Value::Array(_) => Unexpected::Seq,
            Value::Object(_) => Unexpected::Map,
        };
        Err(de::Error::invalid_type(unexpected, &"bytes"))
    }

    fn deserialize_byte_buf<V>(
        self,
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let unexpected = match self.0 {
            Value::Bytes(buffer) => {
                return visitor.visit_bytes(buffer);
            }
            Value::String(s) => return visitor.visit_bytes(s.as_bytes()),
            Value::Null => Unexpected::Unit,
            Value::Boolean(b) => Unexpected::Bool(*b),
            Value::Integer(i) => Unexpected::Signed(*i),
            Value::Float(f) => Unexpected::Float(*f),
            Value::Array(_) => Unexpected::Seq,
            Value::Object(_) => Unexpected::Map,
        };
        Err(de::Error::invalid_type(unexpected, &"bytes"))
    }

    serde::forward_to_deserialize_any! {
        unit bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str
        string identifier ignored_any unit_struct struct map seq
        tuple tuple_struct enum newtype_struct option
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;
    use slumber_util::yaml::deserialize_yaml;

    /// Test deserialization, which has some additional logic on top of parsing
    #[rstest]
    // boolean
    #[case::bool_true(true.into(), "true")]
    #[case::bool_false(false.into(), "false")]
    // numeric
    #[case::u64(1000u64.into(), "1000")]
    #[case::i64_negative((-1000i64).into(), "-1000")]
    #[case::float_positive(10.1.into(), "10.1")]
    #[case::float_negative((-10.1).into(), "-10.1")]
    // string
    #[case::str("hello".into(), "hello")]
    #[case::str_null("null".into(), "null")]
    #[case::str_true("true".into(), "true")]
    #[case::str_false("false".into(), "false")]
    #[case::str_with_keys("{{ user_id }}".into(), "{{ user_id }}")]
    fn test_deserialize_template(
        #[case] value: serde_yaml::Value,
        #[case] expected: &str,
    ) {
        assert_eq!(
            deserialize_yaml::<Template>(value).unwrap(),
            Template::from(expected)
        );
    }
}
