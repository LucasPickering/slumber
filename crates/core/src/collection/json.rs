//! Utilities for working with templated JSON

use crate::render::TemplateContext;
use futures::future;
use indexmap::IndexMap;
use serde::Serialize;
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
    Object(IndexMap<String, Self>),
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
                    .map(|(key, value)| (key, Self::raw(value)))
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
                        let value = value.render(context).await?;
                        Ok::<_, RenderError>((key.clone(), value))
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
            JsonTemplate::Array(json_templates) => serde_json::Value::Array(
                json_templates.iter().map(serde_json::Value::from).collect(),
            ),
            JsonTemplate::Object(index_map) => serde_json::Value::Object(
                index_map
                    .iter()
                    .map(|(key, value)| {
                        (key.clone(), serde_json::Value::from(value))
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
