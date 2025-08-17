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

        let mut deserializer = StructDeserializer::new(yaml)?;

        let config = Self {
            commands: deserializer.get(Field::new("commands").opt())?,
            editor: deserializer.get(Field::new("editor").opt())?,
            pager: deserializer.get(Field::new("pager").opt())?,
            // HTTP config is flattened into the top
            http: HttpEngineConfig {
                ignore_certificate_hosts: deserializer
                    .get(Field::new("ignore_certificate_hosts").opt())?,
                large_body_size: deserializer.get(
                    Field::new("large_body_size")
                        .or(HttpEngineConfig::default().large_body_size),
                )?,
                follow_redirects: deserializer.get(
                    Field::new("follow_redirects")
                        .or(HttpEngineConfig::default().follow_redirects),
                )?,
            },
            preview_templates: deserializer
                .get(Field::new("preview_templates").or(true))?,
            input_bindings: deserializer
                .get(Field::new("input_bindings").opt())?,
            theme: deserializer.get(Field::new("theme").opt())?,
            debug: deserializer.get(Field::new("debug").opt())?,
            persist: deserializer.get(Field::new("persist").or(true))?,
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
        let mut deserializer = StructDeserializer::new(yaml)?;
        let config = Self {
            shell: deserializer.get(Field::new("shell").opt())?,
            default_query: deserializer
                .get(Field::new("default_query").opt())?,
        };
        deserializer.done()?;
        Ok(config)
    }
}

impl DeserializeYaml for HttpEngineConfig {
    fn expected() -> Expected {
        Expected::Mapping
    }

    fn deserialize(yaml: SourcedYaml) -> yaml::Result<Self> {
        let mut deserializer = StructDeserializer::new(yaml)?;
        let config = Self {
            ignore_certificate_hosts: deserializer
                .get(Field::new("ignore_certificate_hosts").opt())?,
            large_body_size: deserializer
                .get(Field::new("large_body_size").opt())?,
            follow_redirects: deserializer
                .get(Field::new("follow_redirects").opt())?,
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
        let mut deserializer = StructDeserializer::new(yaml)?;
        let config = Self {
            primary_color: deserializer
                .get::<Adopt<_>>(Field::new("primary_color").opt())?
                .0,
            primary_text_color: deserializer
                .get::<Adopt<_>>(Field::new("primary_text_color").opt())?
                .0,
            secondary_color: deserializer
                .get::<Adopt<_>>(Field::new("secondary_color").opt())?
                .0,
            success_color: deserializer
                .get::<Adopt<_>>(Field::new("success_color").opt())?
                .0,
            error_color: deserializer
                .get::<Adopt<_>>(Field::new("error_color").opt())?
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
