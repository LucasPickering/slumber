use crate::template::{render::RenderValue, RenderError};
use indexmap::{map, IndexMap};
use serde::{
    de::{
        self, value::StringDeserializer, DeserializeOwned, IntoDeserializer,
        Visitor,
    },
    forward_to_deserialize_any,
};
use std::fmt::Display;

pub fn from_value<T: DeserializeOwned>(
    value: RenderValue,
) -> Result<T, RenderError> {
    T::deserialize(value.into_deserializer())
}

/// TODO
pub struct ValueDeserializer {
    value: RenderValue,
}

impl ValueDeserializer {
    fn new(value: RenderValue) -> ValueDeserializer {
        ValueDeserializer { value }
    }
}

impl<'de> IntoDeserializer<'de, RenderError> for RenderValue {
    type Deserializer = ValueDeserializer;

    fn into_deserializer(self) -> Self::Deserializer {
        ValueDeserializer { value: self }
    }
}

impl<'de> de::Deserializer<'de> for ValueDeserializer {
    type Error = RenderError;

    fn deserialize_any<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: de::Visitor<'de>,
    {
        match self.value {
            RenderValue::Null => visitor.visit_unit(),
            RenderValue::Bool(b) => visitor.visit_bool(b),
            RenderValue::Number(n) => {
                n.deserialize_any(visitor).map_err(de::Error::custom)
            }
            RenderValue::String(s) => visitor.visit_string(s),
            RenderValue::Binary(b) => visitor.visit_bytes(&b),
            RenderValue::Array(array) => {
                visitor.visit_seq(array.into_deserializer())
            }
            RenderValue::Object(object) => {
                visitor.visit_map(object.into_deserializer())
            }
        }
    }

    fn deserialize_option<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        match self.value {
            RenderValue::Null => visitor.visit_none(),
            _ => visitor.visit_some(self),
        }
    }

    fn deserialize_enum<V>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: de::Visitor<'de>,
    {
        match self.value {
            RenderValue::String(s) => visitor.visit_enum(s.into_deserializer()),
            RenderValue::Object(object) => {
                visitor.visit_enum(EnumAccess::new(object))
            }
            _ => Err(de::Error::custom("expected an enum")),
        }
    }

    forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf unit unit_struct newtype_struct seq tuple
        tuple_struct map struct identifier ignored_any
    }
}

impl de::Error for RenderError {
    fn custom<T: Display>(msg: T) -> Self {
        Self::Deserialization(msg.to_string())
    }
}

struct EnumAccess {
    iter: map::IntoIter<String, RenderValue>,
}

impl EnumAccess {
    fn new(map: IndexMap<String, RenderValue>) -> Self {
        EnumAccess {
            iter: map.into_iter(),
        }
    }
}

impl<'de> de::EnumAccess<'de> for EnumAccess {
    type Error = RenderError;
    type Variant = VariantAccess;

    fn variant_seed<V>(
        mut self,
        seed: V,
    ) -> Result<(V::Value, Self::Variant), RenderError>
    where
        V: de::DeserializeSeed<'de>,
    {
        match self.iter.next() {
            Some((value, variant)) => Ok((
                seed.deserialize::<StringDeserializer<RenderError>>(
                    value.into_deserializer(),
                )?,
                VariantAccess::new(variant),
            )),
            None => Err(de::Error::custom("expected an enum variant")),
        }
    }
}

struct VariantAccess {
    value: RenderValue,
}

impl VariantAccess {
    fn new(value: RenderValue) -> Self {
        VariantAccess { value }
    }
}

impl<'de> de::VariantAccess<'de> for VariantAccess {
    type Error = RenderError;

    fn unit_variant(self) -> Result<(), Self::Error> {
        Err(de::Error::custom("expected a string"))
    }

    fn newtype_variant_seed<T>(self, seed: T) -> Result<T::Value, Self::Error>
    where
        T: de::DeserializeSeed<'de>,
    {
        seed.deserialize(ValueDeserializer::new(self.value))
    }

    fn tuple_variant<V>(
        self,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: de::Visitor<'de>,
    {
        de::Deserializer::deserialize_seq(
            ValueDeserializer::new(self.value),
            visitor,
        )
    }

    fn struct_variant<V>(
        self,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: de::Visitor<'de>,
    {
        de::Deserializer::deserialize_map(
            ValueDeserializer::new(self.value),
            visitor,
        )
    }
}
