//! Serialization/deserialization for HTTP-releated types

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

/// Serialization/deserialization for [reqwest::Method]
pub mod serde_method {
    use super::*;
    use reqwest::Method;

    pub fn serialize<S>(
        method: &Method,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(method.as_str())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Method, D::Error>
    where
        D: Deserializer<'de>,
    {
        <&str>::deserialize(deserializer)?
            .parse()
            .map_err(de::Error::custom)
    }
}

/// Serialization/deserialization for [reqwest::header::HeaderMap]
pub mod serde_header_map {
    use super::*;
    use indexmap::IndexMap;
    use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

    pub fn serialize<S>(
        headers: &HeaderMap,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // HeaderValue -> str is fallible, so we'll serialize as bytes instead
        <IndexMap<&str, &[u8]>>::serialize(
            &headers
                .into_iter()
                .map(|(k, v)| (k.as_str(), v.as_bytes()))
                .collect(),
            serializer,
        )
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<HeaderMap, D::Error>
    where
        D: Deserializer<'de>,
    {
        <IndexMap<String, Vec<u8>>>::deserialize(deserializer)?
            .into_iter()
            .map::<Result<(HeaderName, HeaderValue), _>, _>(|(k, v)| {
                // Fallibly map each key and value to header types
                Ok((
                    k.try_into().map_err(de::Error::custom)?,
                    v.try_into().map_err(de::Error::custom)?,
                ))
            })
            .collect()
    }
}

/// Serialization/deserialization for [reqwest::StatusCode]
pub mod serde_status_code {
    use super::*;
    use reqwest::StatusCode;

    pub fn serialize<S>(
        status_code: &StatusCode,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u16(status_code.as_u16())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<StatusCode, D::Error>
    where
        D: Deserializer<'de>,
    {
        StatusCode::from_u16(u16::deserialize(deserializer)?)
            .map_err(de::Error::custom)
    }
}
