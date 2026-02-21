//! Utilities for working with templated JSON

use crate::render::TemplateContext;
use async_trait::async_trait;
use futures::future;
use serde::{Serialize, Serializer, ser::SerializeMap};
use slumber_template::{
    Context, Render, RenderError, Template, TemplateParseError, Value,
    ValueError,
};
use std::str::FromStr;
use thiserror::Error;

/// A JSON value like [serde_json::Value], but all strings are templates
#[derive(Clone, Debug, Serialize)]
#[cfg_attr(any(test, feature = "test"), derive(PartialEq))]
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

    /// Render all templates to strings and return a static JSON value
    ///
    /// TODO implement this as a separate Render impl instead
    pub async fn render_json(
        &self,
        context: &TemplateContext,
    ) -> Result<serde_json::Value, RenderError> {
        let value: Value = self.render(context).await?;
        // Convert the template value to a JSON value via serde
        let json = serde_json::to_value(value).map_err(ValueError::from)?;
        Ok(json)
    }
}

/// TODO
#[async_trait(?Send)]
impl<Ctx: Context> Render<Ctx, Value> for JsonTemplate {
    async fn render(&self, context: &Ctx) -> Result<Value, RenderError> {
        match self {
            JsonTemplate::Null => Ok(Value::Null),
            JsonTemplate::Bool(b) => Ok(Value::Boolean(*b)),
            JsonTemplate::Number(n) => {
                if let Some(i) = n.as_i64() {
                    Ok(Value::Integer(i))
                } else if let Some(f) = n.as_f64() {
                    Ok(Value::Float(f))
                } else {
                    // Integer is out of i64 range. Template values really
                    // should be a superset of JSON values, but right now that's
                    // not the case.
                    Err(RenderError::from(ValueError::other("TODO")))
                }
            }
            // This renderer automatically unpacks the value
            JsonTemplate::String(template) => template.render(context).await,
            JsonTemplate::Array(array) => {
                // Render each value and collection into an Array
                future::try_join_all(
                    array
                        .iter()
                        .map(|value| async { value.render(context).await }),
                )
                .await
                .map(Value::from)
            }
            JsonTemplate::Object(map) => {
                // Render each key/value and collect into an Object
                future::try_join_all(map.iter().map(|(key, value)| async {
                    let key: String = key.render(context).await?;
                    let value: Value = value.render(context).await?;
                    Ok::<_, RenderError>((key, value))
                }))
                .await
                .map(Value::from)
            }
        }
    }
}

// TODO get rid of this once JSON templates aren't previewed fuckily
impl From<&JsonTemplate> for serde_json::Value {
    /// Convert a [JsonTemplate] to a [serde_json::Value], stringifying
    /// templates
    fn from(template: &JsonTemplate) -> Self {
        match template {
            JsonTemplate::Null => serde_json::Value::Null,
            JsonTemplate::Bool(b) => serde_json::Value::Bool(*b),
            JsonTemplate::Number(number) => {
                serde_json::Value::Number(number.clone())
            }
            JsonTemplate::String(template) => {
                serde_json::Value::String(template.display().to_string())
            }
            JsonTemplate::Array(array) => serde_json::Value::Array(
                array.iter().map(serde_json::Value::from).collect(),
            ),
            JsonTemplate::Object(object) => serde_json::Value::Object(
                object
                    .iter()
                    .map(|(key, value)| {
                        (
                            key.display().to_string(),
                            serde_json::Value::from(value),
                        )
                    })
                    .collect(),
            ),
        }
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
