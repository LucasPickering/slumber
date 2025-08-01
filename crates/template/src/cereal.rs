use crate::{RenderError, Template, Value};
use saphyr::{MarkedYaml, Scalar, YamlData};
use serde::{
    Serialize,
    de::{
        self, IntoDeserializer, Unexpected, Visitor,
        value::{MapDeserializer, SeqDeserializer},
    },
};
use slumber_util::yaml::{DeserializeYaml, Expected, LocatedError};

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
        yaml: MarkedYaml,
    ) -> Result<Self, LocatedError<slumber_util::yaml::Error>> {
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
            .map_err(|error| LocatedError::other(error, yaml.span.start))
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
pub struct ValueDeserializer(Value);

impl IntoDeserializer<'_, RenderError> for Value {
    type Deserializer = ValueDeserializer;

    fn into_deserializer(self) -> ValueDeserializer {
        ValueDeserializer(self)
    }
}

impl<'de> serde::Deserializer<'de> for ValueDeserializer {
    type Error = RenderError;

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
            Value::Bytes(buffer) => {
                // In most cases where bytes are returned, the user actually
                // wants a string (e.g. in a JSON value). If we can convert to
                // a string, deserialize as that.
                //
                // If the user actually wants bytes no matter what, the
                // Deserialize impl should call deserialize_bytes or
                // deserialize_byte_buf
                match std::str::from_utf8(&buffer) {
                    Ok(s) => visitor.visit_str(s),
                    Err(_) => visitor.visit_bytes(&buffer),
                }
            }
            Value::Array(array) => {
                visitor.visit_seq(&mut SeqDeserializer::new(array.into_iter()))
            }
            Value::Object(object) => {
                visitor.visit_map(&mut MapDeserializer::new(object.into_iter()))
            }
        }
    }
    fn deserialize_bytes<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let unexpected = match self.0 {
            Value::Bytes(buffer) => return visitor.visit_bytes(&buffer),
            Value::String(s) => return visitor.visit_bytes(s.as_bytes()),
            Value::Null => Unexpected::Unit,
            Value::Bool(b) => Unexpected::Bool(b),
            Value::Int(i) => Unexpected::Signed(i),
            Value::Float(f) => Unexpected::Float(f),
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
                return visitor.visit_byte_buf(buffer.into());
            }
            Value::String(s) => return visitor.visit_byte_buf(s.into_bytes()),
            Value::Null => Unexpected::Unit,
            Value::Bool(b) => Unexpected::Bool(b),
            Value::Int(i) => Unexpected::Signed(i),
            Value::Float(f) => Unexpected::Float(f),
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
