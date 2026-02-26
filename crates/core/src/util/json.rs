//! Utilities for working with templated JSON

use crate::collection::ValueTemplate;
use slumber_template::{
    Context, RenderError, Template, TemplateParseError, Value,
};
use thiserror::Error;

impl ValueTemplate {
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

    /// Get a [ValueTemplate::Number] from a [serde_json::Number]
    pub fn from_json_number(n: serde_json::Number) -> Self {
        if let Some(i) = n.as_i64() {
            Self::Integer(i)
        } else if let Some(f) = n.as_f64() {
            Self::Float(f)
        } else {
            unreachable!(
                "serde_json doesn't support >64-bit numbers with \
                arbitrary_precision disabled"
            );
        }
    }

    /// Parse JSON to a [ValueTemplate]
    ///
    /// The string is parsed to JSON first, then the strings are parsed to
    /// [Template]s. Everything else is mapped 1:1 to its [ValueTemplate]
    /// counterpart variant.
    pub fn parse_json(s: &str) -> Result<Self, JsonTemplateError> {
        // First, parse it as regular JSON
        let json: serde_json::Value = serde_json::from_str(s)?;
        // Then map all the strings as templates
        let mapped = json.try_into()?;
        Ok(mapped)
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

// YAML isn't *exactly* JSON, but it's close so I'm putting YAML stuff here too
impl TryFrom<serde_yaml::Value> for ValueTemplate {
    type Error = YamlTemplateError;

    /// Convert static JSON to templated JSON, parsing each string as a template
    fn try_from(yaml: serde_yaml::Value) -> Result<Self, Self::Error> {
        let mapped = match yaml {
            serde_yaml::Value::Null => Self::Null,
            serde_yaml::Value::Bool(b) => Self::Boolean(b),
            serde_yaml::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    Self::Integer(i)
                } else if let Some(f) = n.as_f64() {
                    Self::Float(f)
                } else {
                    unreachable!("serde_yaml doesn't support >64-bit numbers");
                }
            }
            serde_yaml::Value::String(s) => Self::String(s.parse()?),
            serde_yaml::Value::Sequence(values) => Self::Array(
                values
                    .into_iter()
                    .map(Self::try_from)
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            serde_yaml::Value::Mapping(map) => Self::Object(
                map.into_iter()
                    .map(|(key, value)| {
                        let key = key
                            .as_str()
                            .ok_or_else(|| {
                                YamlTemplateError::InvalidKey(key.clone())
                            })?
                            .parse()?;
                        let value = value.try_into()?;
                        Ok::<_, YamlTemplateError>((key, value))
                    })
                    .collect::<Result<_, _>>()?,
            ),
            serde_yaml::Value::Tagged(value) => value.value.try_into()?,
        };
        Ok(mapped)
    }
}

/// Error that can occur when parsing to from JSON to [ValueTemplate]
#[derive(Debug, Error)]
pub enum JsonTemplateError {
    /// Content was invalid JSON
    #[error(transparent)]
    JsonParse(#[from] serde_json::Error),
    /// Content was valid JSON but one of the contained strings was an invalid
    /// Slumber template
    #[error(transparent)]
    TemplateParse(#[from] TemplateParseError),
}

/// Error that can occur when parsing to YAML to [ProfileTemplate]
#[derive(Debug, Error)]
pub enum YamlTemplateError {
    /// Content was invalid YAML
    #[error(transparent)]
    YamlParse(#[from] serde_yaml::Error),
    /// Content was valid YAML but one of the contained strings was an invalid
    /// Slumber template
    #[error(transparent)]
    TemplateParse(#[from] TemplateParseError),
    /// Mapping had a non-string key, which isn't allowed in Slumber values
    #[error("Mapping keys must be strings, but received: {0:?}")]
    InvalidKey(serde_yaml::Value),
}

/// Convert a template [Value] to a JSON value
pub fn value_to_json(value: Value) -> serde_json::Value {
    match value {
        Value::Null => serde_json::Value::Null,
        Value::Boolean(b) => b.into(),
        Value::Integer(i) => i.into(),
        Value::Float(f) => f.into(),
        Value::String(s) => s.into(),
        Value::Array(array) => array.into_iter().map(value_to_json).collect(),
        Value::Object(object) => object
            .into_iter()
            .map(|(key, value)| (key, value_to_json(value)))
            .collect(),
        // Convert bytes to an int array. This isn't really useful, but it
        // keeps this method infallible which is really nice. And generally
        // it will probably be less disruptive to the user than an error.
        Value::Bytes(bytes) => bytes.to_vec().into(),
    }
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
    use serde_yaml::{
        Mapping,
        value::{Tag, TaggedValue},
    };
    use slumber_util::{Factory, assert_result};

    #[rstest]
    #[case::null(serde_json::Value::Null, ValueTemplate::Null)]
    // Templates aren't parsed
    #[case::valid_template(json!("{{valid}}"), "{_{valid}}".into())]
    #[case::invalid_template(json!("{{invalid"), "{_{invalid".into())]
    fn test_from_raw_json(
        #[case] json: serde_json::Value,
        #[case] expected: ValueTemplate,
    ) {
        assert_eq!(ValueTemplate::from_raw_json(json), expected);
    }

    #[rstest]
    #[case::int(3.into(), 3.into())]
    // Template values use i64, so anything between (i64::MAX, u64::MAX] is
    // converted to a float instead
    #[case::int_too_big(
        serde_json::Number::from(u64::MAX), (u64::MAX as f64).into()
    )]
    #[case::float(serde_json::Number::from_f64(42.9).unwrap(), 42.9.into())]
    fn test_from_json_num(
        #[case] number: serde_json::Number,
        #[case] expected: ValueTemplate,
    ) {
        assert_eq!(ValueTemplate::from_json_number(number), expected);
    }

    /// Parse a string as JSON, then convert to [ValueTemplate]. This uses the
    /// TryFrom impl tested below, so we don't need many cases here
    #[rstest]
    #[case::null("null", Ok(ValueTemplate::Null))]
    #[case::object(r#"{"{{w}}": 3}"#, Ok(vec![("{{ w }}", 3)].into()))]
    #[case::error_invalid_template_key(
        r#"{"{{invalid": 3}"#,
        Err("invalid expression")
    )]
    fn test_parse_json(
        #[case] s: &str,
        #[case] expected: Result<ValueTemplate, &str>,
    ) {
        assert_result(ValueTemplate::parse_json(s), expected);
    }

    /// Test the JSON -> ValueTemplate TryFrom impl
    #[rstest]
    #[case::null(json!(null), Ok(ValueTemplate::Null))]
    #[case::template_string(json!("{{ w }}"), Ok("{{w}}".into()))]
    #[case::template_key(json!({"{{ w }}": 3}), Ok(vec![("{{w}}", 3)].into()))]
    #[case::error_invalid_template_key(
        json!({"{{ invalid_key": {"name": "{{ username }}"}}),
        Err("invalid expression")
    )]
    #[case::error_invalid_template_value(
        json!({"key": "{{ invalid"}), Err("invalid expression")
    )]
    fn test_from_json(
        #[case] json: serde_json::Value,
        #[case] expected: Result<ValueTemplate, &str>,
    ) {
        assert_result(ValueTemplate::try_from(json), expected);
    }

    /// serde_json's `arbitrary_precision` feature is disabled, meaning any int
    /// larger than 64 bits is not supported. This is important because template
    /// values use `i64`/`f64`, so we can't fit all large values.
    #[test]
    fn test_arbitrary_precision_disabled() {
        assert_eq!(
            serde_json::Number::from_i128(i128::from(u64::MAX) + 1),
            None
        );
    }

    /// Test the YAML -> ValueTemplate TryFrom impl
    ///
    /// I'm taking some shortcuts on this because the implementation is very
    /// similar to the JSON one. YAML values are more annoying to construct
    /// because there's no macro, and I don't feel like doing it.
    #[rstest]
    #[case::null(serde_yaml::Value::Null, Ok(ValueTemplate::Null))]
    // Template values use i64, so anything between (i64::MAX, u64::MAX] is
    // converted to a float instead
    #[case::int_too_big(u64::MAX.into(), Ok((u64::MAX as f64).into()))]
    #[case::float(42.9.into(), Ok(42.9.into()))]
    #[case::float_inf(f64::INFINITY.into(), Ok(f64::INFINITY.into()))]
    #[case::float_nan(f64::NAN.into(), Ok(f64::NAN.into()))]
    #[case::template_string("{{ w }}".into(), Ok("{{w}}".into()))]
    #[case::template_key(
        Mapping::from_iter([("{{w}}".into(), 3.into())]).into(),
        Ok(vec![("{{w}}", 3)].into()),
    )]
    #[case::error_invalid_template_key(
        Mapping::from_iter([("{{invalid".into(), 3.into())]).into(),
        Err("invalid expression"),
    )]
    #[case::error_invalid_template_value(
        Mapping::from_iter([(3.into(), "{{invalid".into())]).into(),
        Err("invalid expression"),
    )]
    #[case::tagged(
        // Tags are thrown out, but the inner value is used
        serde_yaml::Value::Tagged(TaggedValue {
            tag: Tag::new("test"),
            value: "{{w}}".into(),
        }.into()),
        Ok("{{ w }}".into()),
    )]
    fn test_from_yaml(
        #[case] yaml: serde_yaml::Value,
        #[case] expected: Result<ValueTemplate, &str>,
    ) {
        assert_result(ValueTemplate::try_from(yaml), expected);
    }

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
}
