//! Utilities for working with templated JSON

use crate::render::TemplateContext;
use futures::future;
use serde::{Serialize, Serializer, ser::SerializeMap};
use slumber_template::{
    RenderError, Template, TemplateParseError, TryFromValue,
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
    pub async fn render(
        &self,
        context: &TemplateContext,
    ) -> Result<serde_json::Value, RenderError> {
        let rendered = match self {
            Self::Null => serde_json::Value::Null,
            Self::Bool(b) => serde_json::Value::Bool(*b),
            Self::Number(number) => serde_json::Value::Number(number.clone()),
            Self::String(template) => {
                // Render to a JSON value instead of just a string. If the
                // template is a single chunk that returns a non-string value
                // (e.g. a number or array), use that value directly. This
                // enables non-string values
                serde_json::Value::try_from_value(
                    template
                        .render(&context.streaming(false))
                        .await
                        .try_collect_value()
                        .await?,
                )
                .map_err(|error| RenderError::Value(error.error))?
            }
            Self::Array(array) => {
                let array = future::try_join_all(
                    array.iter().map(|item| item.render(context)),
                )
                .await?;
                serde_json::Value::Array(array)
            }
            Self::Object(map) => {
                let map = future::try_join_all(map.iter().map(
                    |(key, value)| async {
                        let key = key
                            .render_string(&context.streaming(false))
                            .await?;
                        let value = value.render(context).await?;
                        Ok::<_, RenderError>((key, value))
                    },
                ))
                .await?;
                serde_json::Value::Object(map.into_iter().collect())
            }
        };
        Ok(rendered)
    }
}

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
        collection::Profile, render::TemplateContext, test_util::by_id,
    };
    use indexmap::indexmap;
    use rstest::rstest;
    use serde_json::json;
    use slumber_util::{Factory, assert_result};

    /// Test that object keys are rendered as templates
    #[rstest]
    #[case::template_key(
        json!({"{{ user_id }}": {"name": "{{ username }}"}}),
        Ok(json!({"123": {"name": "testuser"}})),
    )]
    #[case::invalid_key(
        json!({"{{ user_id": {"name": "{{ username }}"}}),
        Err("invalid expression"),
    )]
    #[tokio::test]
    async fn test_render_template_keys(
        #[case] input: serde_json::Value,
        #[case] expected: Result<serde_json::Value, &str>,
    ) {
        let profile = Profile {
            data: indexmap! {
                "user_id".into() => "123".into(),
                "username".into() => "testuser".into()
            },
            ..Profile::factory(())
        };
        let context =
            TemplateContext::factory((by_id([profile]), indexmap! {}));

        let result = match JsonTemplate::try_from(input) {
            // If we're expecting an error, it should happen during the parse
            // so don't check for errors during render. Saves having to
            // consolidate the error types
            Ok(json) => Ok(json.render(&context).await.unwrap()),
            Err(error) => Err(error),
        };
        assert_result(result, expected);
    }
}
