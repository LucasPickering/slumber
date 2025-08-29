//! Deserialization for config types. Unfortunately these have to be handwritten
//! to enable the use of saphyr
//!
//! We can delete this if
//! [saphyr-serde](https://docs.rs/saphyr-serde/latest/saphyr_serde/) gets
//! built.

use crate::{CommandsConfig, Config, HttpEngineConfig, Theme};
use ratatui_core::style::Color;
use serde::de::{self, value::StringDeserializer};
use slumber_util::yaml::{
    self, DeserializeYaml, Expected, Field, LocatedError, SourcedYaml,
    StructDeserializer,
};

impl DeserializeYaml for Config {
    fn expected() -> Expected {
        Expected::Mapping
    }

    fn deserialize(mut yaml: SourcedYaml) -> yaml::Result<Self> {
        // Drop all fields starting with `.`
        yaml.drop_dot_fields();

        let default = Self::default();
        let http_default = HttpEngineConfig::default();
        let mut deserializer = StructDeserializer::new(yaml)?;

        let config = Self {
            commands: deserializer
                .get(Field::new("commands").or(default.commands))?,
            editor: deserializer
                .get(Field::new("editor").or(default.editor))?,
            pager: deserializer.get(Field::new("pager").or(default.pager))?,
            // HTTP config is flattened into the top
            http: HttpEngineConfig {
                ignore_certificate_hosts: deserializer.get(
                    Field::new("ignore_certificate_hosts")
                        .or(http_default.ignore_certificate_hosts),
                )?,
                large_body_size: deserializer.get(
                    Field::new("large_body_size")
                        .or(http_default.large_body_size),
                )?,
                follow_redirects: deserializer.get(
                    Field::new("follow_redirects")
                        .or(http_default.follow_redirects),
                )?,
            },
            preview_templates: deserializer.get(
                Field::new("preview_templates").or(default.preview_templates),
            )?,
            input_bindings: deserializer
                .get(Field::new("input_bindings").or(default.input_bindings))?,
            theme: deserializer.get(Field::new("theme").or(default.theme))?,
            debug: deserializer.get(Field::new("debug").or(default.debug))?,
            persist: deserializer
                .get(Field::new("persist").or(default.persist))?,
        };
        deserializer.done()?;
        Ok(config)
    }
}

impl DeserializeYaml for CommandsConfig {
    fn expected() -> Expected {
        Expected::Mapping
    }

    fn deserialize(yaml: SourcedYaml) -> yaml::Result<Self> {
        let default = Self::default();
        let mut deserializer = StructDeserializer::new(yaml)?;
        let config = Self {
            shell: deserializer.get(Field::new("shell").or(default.shell))?,
            default_query: deserializer
                .get(Field::new("default_query").or(default.default_query))?,
        };
        deserializer.done()?;
        Ok(config)
    }
}

impl DeserializeYaml for Theme {
    fn expected() -> Expected {
        Expected::Mapping
    }

    fn deserialize(yaml: SourcedYaml) -> yaml::Result<Self> {
        let default = Self::default();
        let mut deserializer = StructDeserializer::new(yaml)?;
        let config = Self {
            primary_color: deserializer
                .get::<Adopt<_>>(
                    Field::new("primary_color")
                        .or(Adopt(default.primary_color)),
                )?
                .0,
            primary_text_color: deserializer
                .get::<Adopt<_>>(
                    Field::new("primary_text_color")
                        .or(Adopt(default.primary_text_color)),
                )?
                .0,
            secondary_color: deserializer
                .get::<Adopt<_>>(
                    Field::new("secondary_color")
                        .or(Adopt(default.secondary_color)),
                )?
                .0,
            success_color: deserializer
                .get::<Adopt<_>>(
                    Field::new("success_color")
                        .or(Adopt(default.success_color)),
                )?
                .0,
            error_color: deserializer
                .get::<Adopt<_>>(
                    Field::new("error_color").or(Adopt(default.error_color)),
                )?
                .0,
        };
        deserializer.done()?;
        Ok(config)
    }
}

/// Workaround for the orphan rule
#[derive(Debug, Default)]
struct Adopt<T>(T);

impl DeserializeYaml for Adopt<Color> {
    fn expected() -> Expected {
        Expected::String
    }

    fn deserialize(yaml: SourcedYaml) -> yaml::Result<Self> {
        let location = yaml.location;
        let s = yaml.try_into_string()?;
        // Use the serde implementation for backward compatibility
        <Color as de::Deserialize>::deserialize(StringDeserializer::new(s))
            .map(Adopt)
            .map_err(|error: de::value::Error| {
                LocatedError::other(error, location)
            })
    }
}
