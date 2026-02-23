//! Utilities for working with templated JSON

use crate::{render::TemplateContext, util::value_to_json};
use futures::future;
use serde::{Serialize, Serializer, ser::SerializeMap};
use slumber_template::{
    RenderError, RenderedOutput, Template, TemplateParseError, Value,
    ValueError,
};
use std::str::FromStr;
use thiserror::Error;

/// A JSON value like [serde_json::Value], but all strings are templates
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum JsonTemplate {
    Null,
    Bool(bool),
    Number(serde_json::Number),
    String(Template),
    Array(Vec<Self>),
    // A key-value mapping. Stored as a `Vec` instead of `IndexMap` because
    // the keys are templates, which aren't hashable. We never do key lookups
    // on this so there's no need for a map anyway.
    #[serde(serialize_with = "serialize_object")]
    #[cfg_attr(
        feature = "schema",
        schemars(with = "std::collections::HashMap<Template, Self>")
    )]
    Object(Vec<(Template, Self)>),
}

impl JsonTemplate {
    /// Build a JSON value without parsing strings as templates
    pub fn raw(json: serde_json::Value) -> Self {
        match json {
            serde_json::Value::Null => Self::Null,
            serde_json::Value::Bool(b) => Self::Bool(b),
            serde_json::Value::Number(number) => Self::Number(number),
            serde_json::Value::String(s) => Self::String(Template::raw(s)),
            serde_json::Value::Array(values) => {
                Self::Array(values.into_iter().map(Self::raw).collect())
            }
            serde_json::Value::Object(map) => Self::Object(
                map.into_iter()
                    .map(|(key, value)| (Template::raw(key), Self::raw(value)))
                    .collect(),
            ),
        }
    }

    /// Render to previewable chunks
    ///
    /// The return value is *usually* a single chunk, but if the JSON value is
    /// a multi-chunk template string, then its multi-chunk output will be the
    /// output for this.
    pub async fn render(&self, context: &TemplateContext) -> RenderedOutput {
        match self {
            JsonTemplate::Null => Value::Null.into(),
            JsonTemplate::Bool(b) => Value::Boolean(*b).into(),
            JsonTemplate::Number(n) => {
                if let Some(i) = n.as_i64() {
                    Value::Integer(i).into()
                } else if let Some(f) = n.as_f64() {
                    Value::Float(f).into()
                } else {
                    // Integer is out of i64 range. Template values really
                    // should be a superset of JSON values, but right now that's
                    // not the case.
                    Err(RenderError::from(ValueError::other(
                        "JSON integer out of range",
                    )))
                    .into()
                }
            }
            JsonTemplate::String(template) => template.render(context).await,
            JsonTemplate::Array(array) => {
                // Render each value and collection into an Array
                future::try_join_all(array.iter().map(|value| async {
                    value.render(context).await.try_collect_value().await
                }))
                .await
                .map(Value::from)
                .into() // Wrap into RenderedOutput
            }
            JsonTemplate::Object(map) => {
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
    pub async fn render_json(
        &self,
        context: &TemplateContext,
    ) -> Result<serde_json::Value, RenderError> {
        // Collect render output as a single value. The renderer should always
        // output a single chunk, so it gets unpacked back to one value.
        let value = self.render(context).await.try_collect_value().await?;
        Ok(value_to_json(value))
    }
}

impl FromStr for JsonTemplate {
    type Err = JsonTemplateError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // First, parse it as regular JSON
        let json: serde_json::Value = serde_json::from_str(s)?;
        // Then map all the strings as templates
        let mapped = json.try_into()?;
        Ok(mapped)
    }
}

impl TryFrom<serde_json::Value> for JsonTemplate {
    type Error = TemplateParseError;

    /// Convert static JSON to templated JSON, parsing each string as a template
    fn try_from(json: serde_json::Value) -> Result<Self, Self::Error> {
        let mapped = match json {
            serde_json::Value::Null => Self::Null,
            serde_json::Value::Bool(b) => Self::Bool(b),
            serde_json::Value::Number(number) => Self::Number(number),
            serde_json::Value::String(s) => Self::String(s.parse()?),
            serde_json::Value::Array(values) => Self::Array(
                values
                    .into_iter()
                    .map(JsonTemplate::try_from)
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

#[cfg(any(test, feature = "test"))]
impl From<&'static str> for JsonTemplate {
    fn from(value: &'static str) -> Self {
        Self::String(value.into())
    }
}

/// Serialize a JSON object as a mapping. The derived impl serializes as a
/// sequence
fn serialize_object<S>(
    object: &Vec<(Template, JsonTemplate)>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
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
                "invalid_utf8".into() => invalid_utf8(),
            },
            ..Profile::factory(())
        };
        let context =
            TemplateContext::factory((by_id([profile]), indexmap! {}));

        let template = JsonTemplate::try_from(input).expect("Invalid template");
        let result = template.render_json(&context).await;
        assert_result(result, expected);
    }

    /// Parsing a JSON value with a key that isn't a valid template is an error
    #[test]
    fn test_invalid_key_template() {
        let json = json!({"{{ invalid_key": {"name": "{{ username }}"}});
        assert_result(JsonTemplate::try_from(json), Err("invalid expression"));
    }
}
