//! Deserialization for config types. Unfortunately these have to be handwritten
//! to enable the use of saphyr
//!
//! We can delete this if
//! [saphyr-serde](https://docs.rs/saphyr-serde/latest/saphyr_serde/) gets
//! built.

use crate::{CommandsConfig, Config, HttpEngineConfig, Theme, mime::MimeMap};
use slumber_util::yaml::{
    self, DeserializeYaml, Expected, Field, SourcedYaml, StructDeserializer,
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
            http: deserializer.get(Field::new("http").opt())?,
            preview_templates: deserializer
                .get(Field::new("preview_templates").opt())?,
            input_bindings: deserializer
                .get(Field::new("input_bindings").opt())?,
            theme: deserializer.get(Field::new("theme").opt())?,
            debug: deserializer.get(Field::new("debug").opt())?,
            persist: deserializer.get(Field::new("persist").opt())?,
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

impl DeserializeYaml for MimeMap<String> {
    fn expected() -> Expected {
        Expected::Mapping
    }

    fn deserialize(yaml: SourcedYaml) -> yaml::Result<Self> {
        todo!()
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
                .get(Field::new("primary_color").opt())?,
            primary_text_color: deserializer
                .get(Field::new("primary_text_color").opt())?,
            secondary_color: deserializer
                .get(Field::new("secondary_color").opt())?,
            success_color: deserializer
                .get(Field::new("success_color").opt())?,
            error_color: deserializer.get(Field::new("error_color").opt())?,
        };
        deserializer.done()?;
        Ok(config)
    }
}
