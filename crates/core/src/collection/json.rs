//! Utilities for working with templated JSON
//!
//! TODO rename this file and update comment^^

use crate::util::value_to_json;
use futures::future;
use serde::{Serialize, Serializer, ser::SerializeMap};
use serde_json::Number;
use slumber_template::{
    Context, RenderError, RenderedOutput, Template, TemplateParseError, Value,
};
use thiserror::Error;

/// TODO comment
#[derive(Clone, Debug, derive_more::From, PartialEq, Serialize)]
#[serde(untagged)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum ValueTemplate {
    Null,
    Boolean(bool),
    Integer(i64),
    Float(f64),
    String(Template),
    #[from(ignore)]
    Array(Vec<Self>),
    // A key-value mapping. Stored as a `Vec` instead of `IndexMap` because
    // the keys are templates, which aren't hashable. We never do key lookups
    // on this so there's no need for a map anyway.
    #[from(ignore)]
    #[serde(serialize_with = "serialize_object")]
    #[cfg_attr(
        feature = "schema",
        schemars(with = "std::collections::HashMap<Template, Self>")
    )]
    Object(Vec<(Template, Self)>),
}

impl ValueTemplate {
    /// Create a new string template from a raw string, without parsing it at
    /// all
    ///
    /// Useful when importing from external formats where the string isn't
    /// expected to be a valid Slumber template
    pub fn raw(template: String) -> Self {
        Self::String(Template::raw(template))
    }

    /// Build a JSON value without parsing strings as templates
    pub fn from_raw_json(json: serde_json::Value) -> Self {
        match json {
            serde_json::Value::Null => Self::Null,
            serde_json::Value::Bool(b) => Self::Boolean(b),
            serde_json::Value::Number(n) => Self::from_json_number(n),
            serde_json::Value::String(s) => Self::String(Template::raw(s)),
            serde_json::Value::Array(values) => Self::Array(
                values.into_iter().map(Self::from_raw_json).collect(),
            ),
            serde_json::Value::Object(map) => Self::Object(
                map.into_iter()
                    .map(|(key, value)| {
                        (Template::raw(key), Self::from_raw_json(value))
                    })
                    .collect(),
            ),
        }
    }

    /// TODO
    pub fn from_json_number(n: Number) -> Self {
        if let Some(i) = n.as_i64() {
            Self::Integer(i)
        } else if let Some(f) = n.as_f64() {
            Self::Float(f)
        } else {
            todo!("integer out of range")
        }
    }

    /// TODO
    pub fn parse_json(s: &str) -> Result<Self, JsonTemplateError> {
        // First, parse it as regular JSON
        let json: serde_json::Value = serde_json::from_str(s)?;
        // Then map all the strings as templates
        let mapped = json.try_into()?;
        Ok(mapped)
    }

    /// TODO
    pub fn to_raw_json(&self) -> serde_json::Value {
        match self {
            ValueTemplate::Null => serde_json::Value::Null,
            ValueTemplate::Boolean(b) => (*b).into(),
            ValueTemplate::Integer(i) => (*i).into(),
            ValueTemplate::Float(f) => (*f).into(),
            ValueTemplate::String(template) => {
                serde_json::Value::String(template.display().to_string())
            }
            ValueTemplate::Array(array) => serde_json::Value::Array(
                array.iter().map(Self::to_raw_json).collect(),
            ),
            ValueTemplate::Object(object) => serde_json::Value::Object(
                object
                    .iter()
                    .map(|(key, value)| {
                        (key.display().to_string(), Self::to_raw_json(value))
                    })
                    .collect(),
            ),
        }
    }

    /// Does the template have at least one dynamic chunk? If this returns
    /// `false`, the template will always render to its source text
    pub fn is_dynamic(&self) -> bool {
        match self {
            Self::Null
            | Self::Boolean(_)
            | Self::Integer(_)
            | Self::Float(_) => false,
            Self::String(template) => template.is_dynamic(),
            Self::Array(array) => array.iter().any(Self::is_dynamic),
            Self::Object(object) => object
                .iter()
                .any(|(key, value)| key.is_dynamic() || value.is_dynamic()),
        }
    }
    /// Render to previewable chunks
    ///
    /// The return value is *usually* a single chunk, but if the JSON value is
    /// a multi-chunk template string, then its multi-chunk output will be the
    /// output for this.
    ///
    /// TODO update comment
    pub async fn render<Ctx: Context>(&self, context: &Ctx) -> RenderedOutput {
        match self {
            Self::Null => Value::Null.into(),
            Self::Boolean(b) => Value::Boolean(*b).into(),
            Self::Integer(i) => Value::Integer(*i).into(),
            Self::Float(f) => Value::Float(*f).into(),
            Self::String(template) => template.render(context).await,
            Self::Array(array) => {
                // Render each value and collection into an Array
                future::try_join_all(array.iter().map(|value| async {
                    value.render(context).await.try_collect_value().await
                }))
                .await
                .map(Value::from)
                .into() // Wrap into RenderedOutput
            }
            Self::Object(map) => {
                // Render each key/value and collect into an Object
                future::try_join_all(map.iter().map(|(key, value)| async {
                    let key = key.render_string(context).await?;
                    let value =
                        value.render(context).await.try_collect_value().await?;
                    Ok::<_, RenderError>((key, value))
                }))
                .await
                .map(Value::from)
                .into() // Wrap into RenderedOutput
            }
        }
    }

    /// Render all templates to strings and return a static JSON value
    pub async fn render_json<Ctx: Context>(
        &self,
        context: &Ctx,
    ) -> Result<serde_json::Value, RenderError> {
        // Collect render output as a single value. The renderer should always
        // output a single chunk, so it gets unpacked back to one value.
        let value = self.render(context).await.try_collect_value().await?;
        Ok(value_to_json(value))
    }
}

impl TryFrom<serde_json::Value> for ValueTemplate {
    type Error = TemplateParseError;

    /// Convert static JSON to templated JSON, parsing each string as a template
    fn try_from(json: serde_json::Value) -> Result<Self, Self::Error> {
        let mapped = match json {
            // Primitive values are always static, so we can re-use raw_json()
            primitive @ (serde_json::Value::Null
            | serde_json::Value::Bool(_)
            | serde_json::Value::Number(_)) => {
                ValueTemplate::from_raw_json(primitive)
            }
            // These values could all potentially be dynamic
            serde_json::Value::String(s) => Self::String(s.parse()?),
            serde_json::Value::Array(values) => Self::Array(
                values
                    .into_iter()
                    .map(Self::try_from)
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            serde_json::Value::Object(map) => Self::Object(
                map.into_iter()
                    .map(|(key, value)| {
                        let key = key.parse()?;
                        let value = value.try_into()?;
                        Ok::<_, TemplateParseError>((key, value))
                    })
                    .collect::<Result<_, _>>()?,
            ),
        };
        Ok(mapped)
    }
}

/// Parse template from a string literal. Panic if invalid
#[cfg(any(test, feature = "test"))]
impl From<&str> for ValueTemplate {
    fn from(value: &str) -> Self {
        let template = value.parse().unwrap();
        Self::String(template)
    }
}

#[cfg(any(test, feature = "test"))]
impl<T: Into<ValueTemplate>> From<Vec<T>> for ValueTemplate {
    fn from(value: Vec<T>) -> Self {
        Self::Array(value.into_iter().map(T::into).collect())
    }
}

#[cfg(any(test, feature = "test"))]
impl<T: Into<ValueTemplate>> From<Vec<(&str, T)>> for ValueTemplate {
    fn from(value: Vec<(&str, T)>) -> Self {
        Self::Object(
            value
                .into_iter()
                .map(|(k, v)| (k.parse().unwrap(), v.into()))
                .collect(),
        )
    }
}

/// Serialize a JSON object as a mapping. The derived impl serializes as a
/// sequence
///
/// TODO make this static instead of generic once JsonTemplate and ValueTemplate
/// are merged
fn serialize_object<S, T>(
    object: &Vec<(Template, T)>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
    T: Serialize,
{
    let mut map = serializer.serialize_map(Some(object.len()))?;
    for (k, v) in object {
        map.serialize_entry(k, v)?;
    }
    map.end()
}

/// Error that can occur when parsing to [JsonTemplate]
#[derive(Debug, Error)]
pub enum JsonTemplateError {
    /// Content was invalid JSON
    #[error(transparent)]
    JsonParse(#[from] serde_json::Error),
    /// Content was valid JSON but one of the contained strings was an invalid
    /// template
    #[error(transparent)]
    TemplateParse(#[from] TemplateParseError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        collection::Profile,
        render::TemplateContext,
        test_util::{by_id, invalid_utf8},
    };
    use indexmap::indexmap;
    use rstest::rstest;
    use serde_json::json;
    use slumber_util::{Factory, assert_result};

    // TODO add JSON number conversion test cases
    // - Slumber -> JSON: Inf, -Inf, NaN
    // - JSON -> Slumber: i64 out of range

    /// Render JSON templates to JSON values
    #[rstest]
    #[case::null(json!(null), Ok(json!(null)))]
    #[case::bool(json!(true), Ok(json!(true)))]
    #[case::integer(json!(3), Ok(json!(3)))]
    #[case::float(json!(3.15), Ok(json!(3.15)))]
    #[case::string(json!("{{ username }}"), Ok(json!("testuser")))]
    #[case::array(
        json!([1, 2, "{{ username }}"]),
        Ok(json!([1, 2, "testuser"])),
    )]
    #[case::object(
        json!({"{{ user_id }}": {"name": "{{ username }}"}}),
        Ok(json!({"123": {"name": "testuser"}})),
    )]
    // serde_json converts the byte string to a number array. Seems reasonable
    // enough.
    #[case::bytes(json!("{{ invalid_utf8 }}"), Ok(json!([0xc3, 0x28])))]
    // Once we have non-string profile values we can test:
    // - Invalid int/float values
    // - Unpacking strings
    #[tokio::test]
    async fn test_render_json(
        #[case] input: serde_json::Value,
        #[case] expected: Result<serde_json::Value, &str>,
    ) {
        let profile = Profile {
            data: indexmap! {
                "user_id".into() => "123".into(),
                "username".into() => "testuser".into(),
                "invalid_utf8".into() => invalid_utf8().into(),
            },
            ..Profile::factory(())
        };
        let context =
            TemplateContext::factory((by_id([profile]), indexmap! {}));

        let template =
            ValueTemplate::try_from(input).expect("Invalid template");
        let result = template.render_json(&context).await;
        assert_result(result, expected);
    }

    /// Parsing a JSON value with a key that isn't a valid template is an error
    #[test]
    fn test_invalid_key_template() {
        let json = json!({"{{ invalid_key": {"name": "{{ username }}"}});
        assert_result(ValueTemplate::try_from(json), Err("invalid expression"));
    }
}
