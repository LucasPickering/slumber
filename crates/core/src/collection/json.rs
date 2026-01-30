//! Utilities for working with templated JSON

use crate::render::SingleRenderContext;
use futures::future;
use serde::{Serialize, Serializer, ser::SerializeMap};
use slumber_template::{
    LazyValue, RenderError, Template, TemplateParseError, Value,
};
use thiserror::Error;

/// TODO comment
/// TODO move this
#[derive(Clone, Debug, Serialize)]
#[cfg_attr(any(test, feature = "test"), derive(derive_more::From, PartialEq))]
#[serde(untagged)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum ValueTemplate {
    Null,
    Boolean(bool),
    Integer(i64),
    Float(f64),
    String(Template),
    #[cfg_attr(any(test, feature = "test"), from(ignore))]
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

impl ValueTemplate {
    /// Build a JSON value without parsing strings as templates
    pub fn raw_json(json: serde_json::Value) -> Self {
        match json {
            serde_json::Value::Null => Self::Null,
            serde_json::Value::Bool(b) => Self::Boolean(b),
            serde_json::Value::Number(_) => todo!(),
            serde_json::Value::String(s) => Self::String(Template::raw(s)),
            serde_json::Value::Array(values) => {
                Self::Array(values.into_iter().map(Self::raw_json).collect())
            }
            serde_json::Value::Object(map) => Self::Object(
                map.into_iter()
                    .map(|(key, value)| {
                        (Template::raw(key), Self::raw_json(value))
                    })
                    .collect(),
            ),
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
    pub async fn render(
        &self,
        context: &SingleRenderContext<'_>,
    ) -> Result<LazyValue, RenderError> {
        match self {
            Self::Null => Ok(Value::Null.into()),
            Self::Boolean(b) => Ok(Value::Boolean(*b).into()),
            Self::Integer(i) => Ok(Value::Integer(*i).into()),
            Self::Float(f) => Ok(Value::Float(*f).into()),
            Self::String(template) => {
                Ok(template.render(context).await.unpack())
            }
            Self::Array(array) => {
                // TODO explain
                let values =
                    future::try_join_all(array.iter().map(|value| async {
                        value
                            .render(&context.streaming(false))
                            .await?
                            .resolve()
                            .await
                    }))
                    .await?;
                Ok(values.into())
            }
            Self::Object(map) => {
                // TODO explain
                let entries = future::try_join_all(map.iter().map(
                    |(key, value)| async {
                        let key = key
                            .render_string(&context.streaming(false))
                            .await?;
                        let value = value
                            .render(&context.streaming(false))
                            .await?
                            .resolve()
                            .await?;
                        Ok::<_, RenderError>((key, value))
                    },
                ))
                .await?;
                Ok(entries.into())
            }
        }
    }
}

impl TryFrom<serde_json::Value> for ValueTemplate {
    type Error = TemplateParseError;

    /// Convert static JSON to templated JSON, parsing each string as a template
    fn try_from(json: serde_json::Value) -> Result<Self, Self::Error> {
        let mapped = match json {
            serde_json::Value::Null => Self::Null,
            serde_json::Value::Bool(b) => Self::Boolean(b),
            serde_json::Value::Number(_) => todo!(),
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
        Ok(json!({"123": {"name": "testuser"}}).into()),
    )]
    #[case::invalid_key(
        json!({"{{ user_id": {"name": "{{ username }}"}}),
        Err("invalid expression"),
    )]
    #[tokio::test]
    async fn test_render_json_template_keys(
        #[case] input: serde_json::Value,
        #[case] expected: Result<slumber_template::Value, &str>,
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

        let result = match ValueTemplate::try_from(input) {
            // If we're expecting an error, it should happen during the parse
            // so don't check for errors during render. Saves having to
            // consolidate the error types
            Ok(json) => Ok(json
                .render(&context.streaming(false))
                .await
                .unwrap()
                .resolve()
                .await
                .unwrap()),
            Err(error) => Err(error),
        };
        assert_result(result, expected);
    }
}
