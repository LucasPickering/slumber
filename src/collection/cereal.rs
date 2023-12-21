//! Serialization/deserialization helpers for collection types

use crate::{
    collection::{
        Chain, ChainId, Profile, ProfileId, ProfileValue, Recipe, RecipeId,
    },
    template::Template,
};
use serde::{
    de::{EnumAccess, VariantAccess},
    Deserialize, Deserializer,
};
use std::{fmt, hash::Hash, marker::PhantomData};

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

impl HasId for Recipe {
    type Id = RecipeId;

    fn set_id(&mut self, id: Self::Id) {
        self.id = id;
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

/// Deserialize a string OR enum into a ProfileValue. This is based on the
/// generated derive code, with extra logic to default to !raw for a string.
impl<'de> Deserialize<'de> for ProfileValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        const VARIANTS: &[&str] = &["raw", "template"];

        enum Field {
            Raw,
            Template,
        }

        struct FieldVisitor;
        impl<'de> serde::de::Visitor<'de> for FieldVisitor {
            type Value = Field;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(f, "variant identifier")
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                match value {
                    0u64 => Ok(Field::Raw),
                    1u64 => Ok(Field::Template),
                    _ => Err(serde::de::Error::invalid_value(
                        serde::de::Unexpected::Unsigned(value),
                        &"variant index 0 <= i < 2",
                    )),
                }
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                match value {
                    "raw" => Ok(Field::Raw),
                    "template" => Ok(Field::Template),
                    _ => {
                        Err(serde::de::Error::unknown_variant(value, VARIANTS))
                    }
                }
            }

            fn visit_bytes<E>(self, value: &[u8]) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                match value {
                    b"raw" => Ok(Field::Raw),
                    b"template" => Ok(Field::Template),
                    _ => {
                        let value = String::from_utf8_lossy(value);
                        Err(serde::de::Error::unknown_variant(&value, VARIANTS))
                    }
                }
            }
        }

        impl<'de> serde::Deserialize<'de> for Field {
            #[inline]
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                serde::Deserializer::deserialize_identifier(
                    deserializer,
                    FieldVisitor,
                )
            }
        }

        struct Visitor<'de> {
            lifetime: PhantomData<&'de ()>,
        }

        impl<'de> serde::de::Visitor<'de> for Visitor<'de> {
            type Value = ProfileValue;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(f, "enum ProfileValue or string",)
            }

            fn visit_enum<A>(self, data: A) -> Result<Self::Value, A::Error>
            where
                A: EnumAccess<'de>,
            {
                match EnumAccess::variant(data)? {
                    (Field::Raw, variant) => Result::map(
                        VariantAccess::newtype_variant::<String>(variant),
                        ProfileValue::Raw,
                    ),
                    (Field::Template, variant) => Result::map(
                        VariantAccess::newtype_variant::<Template>(variant),
                        ProfileValue::Template,
                    ),
                }
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(ProfileValue::Raw(value.into()))
            }

            fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(ProfileValue::Raw(value))
            }
        }

        deserializer.deserialize_any(Visitor {
            lifetime: PhantomData,
        })
    }
}
