//! Deserialization for config types. Unfortunately these have to be handwritten
//! to enable the use of saphyr
//!
//! We can delete this if
//! [saphyr-serde](https://docs.rs/saphyr-serde/latest/saphyr_serde/) gets
//! built.

use crate::{Config, HttpEngineConfig};
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
            // Both these configs get flattened to the top, so they share the
            // same deserializer
            http: deserialize_http_config(&mut deserializer)?,
            #[cfg(feature = "tui")]
            tui: tui::deserialize_tui_config(&mut deserializer)?,
        };

        // If we're not running in TUI mode, we know there's still TUI fields
        // in the YAML, so we don't want to fail for those. If all fields are
        // enabled though, extraneous fields should error.
        #[cfg(feature = "tui")]
        {
            deserializer.done()?;
        }

        Ok(config)
    }
}

/// Deserialize HTTP-specific config fields from an existing deserializer
fn deserialize_http_config(
    deserializer: &mut StructDeserializer,
) -> yaml::Result<HttpEngineConfig> {
    let default = HttpEngineConfig::default();
    Ok(HttpEngineConfig {
        ignore_certificate_hosts: deserializer.get(
            Field::new("ignore_certificate_hosts")
                .or(default.ignore_certificate_hosts),
        )?,
        large_body_size: deserializer
            .get(Field::new("large_body_size").or(default.large_body_size))?,
        follow_redirects: deserializer
            .get(Field::new("follow_redirects").or(default.follow_redirects))?,
    })
}

/// TUI-specific config deserialization
#[cfg(feature = "tui")]
mod tui {
    use crate::tui::{CommandsConfig, Theme, TuiConfig};
    use ratatui_core::style::Color;
    use serde::de::{self, value::StringDeserializer};
    use slumber_util::yaml::{
        self, DeserializeYaml, Expected, Field, LocatedError, SourcedYaml,
        StructDeserializer,
    };

    /// Deserialize TUI-specific config fields from an existing deserializer
    pub fn deserialize_tui_config(
        deserializer: &mut StructDeserializer,
    ) -> yaml::Result<TuiConfig> {
        let default = TuiConfig::default();
        Ok(TuiConfig {
            commands: deserializer
                .get(Field::new("commands").or(default.commands))?,
            editor: deserializer
                .get(Field::new("editor").or(default.editor))?,
            pager: deserializer.get(Field::new("pager").or(default.pager))?,
            preview_templates: deserializer.get(
                Field::new("preview_templates").or(default.preview_templates),
            )?,
            input_bindings: deserializer
                .get(Field::new("input_bindings").or(default.input_bindings))?,
            theme: deserializer.get(Field::new("theme").or(default.theme))?,
            debug: deserializer.get(Field::new("debug").or(default.debug))?,
            persist: deserializer
                .get(Field::new("persist").or(default.persist))?,
        })
    }

    impl DeserializeYaml for CommandsConfig {
        fn expected() -> Expected {
            Expected::Mapping
        }

        fn deserialize(yaml: SourcedYaml) -> yaml::Result<Self> {
            let default = Self::default();
            let mut deserializer = StructDeserializer::new(yaml)?;
            let config = Self {
                shell: deserializer
                    .get(Field::new("shell").or(default.shell))?,
                default_query: deserializer.get(
                    Field::new("default_query").or(default.default_query),
                )?,
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
                        Field::new("error_color")
                            .or(Adopt(default.error_color)),
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
}
