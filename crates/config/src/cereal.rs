//! Deserialization for config types. Unfortunately these have to be handwritten
//! to enable the use of saphyr
//!
//! We can delete this if
//! [saphyr-serde](https://docs.rs/saphyr-serde/latest/saphyr_serde/) gets
//! built.

use crate::{Config, HttpEngineConfig};
use slumber_util::yaml::{
    self, DeserializeYaml, Expected, Field, SourceMap, SourcedYaml,
    StructDeserializer,
};

impl DeserializeYaml for Config {
    fn expected() -> Expected {
        Expected::Mapping
    }

    fn deserialize(
        mut yaml: SourcedYaml,
        source_map: &SourceMap,
    ) -> yaml::Result<Self> {
        // Drop all fields starting with `.`
        yaml.drop_dot_fields();

        let default = Self::default();
        let mut deserializer = StructDeserializer::new(yaml)?;

        let config = Self {
            editor: deserializer
                .get(Field::new("editor").or(default.editor), source_map)?,
            // Both these configs get flattened to the top, so they share the
            // same deserializer
            http: deserialize_http_config(&mut deserializer, source_map)?,
            #[cfg(feature = "tui")]
            tui: tui::deserialize_tui_config(&mut deserializer, source_map)?,
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
    source_map: &SourceMap,
) -> yaml::Result<HttpEngineConfig> {
    let default = HttpEngineConfig::default();
    Ok(HttpEngineConfig {
        ignore_certificate_hosts: deserializer.get(
            Field::new("ignore_certificate_hosts")
                .or(default.ignore_certificate_hosts),
            source_map,
        )?,
        large_body_size: deserializer.get(
            Field::new("large_body_size").or(default.large_body_size),
            source_map,
        )?,
        follow_redirects: deserializer.get(
            Field::new("follow_redirects").or(default.follow_redirects),
            source_map,
        )?,
    })
}

/// TUI-specific config deserialization
#[cfg(feature = "tui")]
mod tui {
    use crate::tui::{CommandsConfig, Syntax, Theme, TuiConfig};
    use ratatui_core::style::Color;
    use serde::de::{self, value::StringDeserializer};
    use slumber_util::yaml::{
        self, DeserializeYaml, Expected, Field, LocatedError, SourceMap,
        SourcedYaml, StructDeserializer,
    };

    /// Deserialize TUI-specific config fields from an existing deserializer
    pub fn deserialize_tui_config(
        deserializer: &mut StructDeserializer,
        source_map: &SourceMap,
    ) -> yaml::Result<TuiConfig> {
        let default = TuiConfig::default();
        Ok(TuiConfig {
            commands: deserializer
                .get(Field::new("commands").or(default.commands), source_map)?,
            pager: deserializer
                .get(Field::new("pager").or(default.pager), source_map)?,
            preview_templates: deserializer.get(
                Field::new("preview_templates").or(default.preview_templates),
                source_map,
            )?,
            input_bindings: deserializer.get(
                Field::new("input_bindings").or(default.input_bindings),
                source_map,
            )?,
            theme: deserializer
                .get(Field::new("theme").or(default.theme), source_map)?,
            debug: deserializer
                .get(Field::new("debug").or(default.debug), source_map)?,
            persist: deserializer
                .get(Field::new("persist").or(default.persist), source_map)?,
        })
    }

    impl DeserializeYaml for CommandsConfig {
        fn expected() -> Expected {
            Expected::Mapping
        }

        fn deserialize(
            yaml: SourcedYaml,
            source_map: &SourceMap,
        ) -> yaml::Result<Self> {
            let default = Self::default();
            let mut deserializer = StructDeserializer::new(yaml)?;
            let config = Self {
                shell: deserializer
                    .get(Field::new("shell").or(default.shell), source_map)?,
                default_query: deserializer.get(
                    Field::new("default_query").or(default.default_query),
                    source_map,
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

        fn deserialize(
            yaml: SourcedYaml,
            source_map: &SourceMap,
        ) -> yaml::Result<Self> {
            let default = Self::default();
            let mut deserializer = StructDeserializer::new(yaml)?;
            let config = Self {
                primary_color: deserializer
                    .get::<Adopt<_>>(
                        Field::new("primary_color")
                            .or(Adopt(default.primary_color)),
                        source_map,
                    )?
                    .0,
                inactive_color: deserializer
                    .get::<Adopt<_>>(
                        Field::new("inactive_color")
                            .or(Adopt(default.inactive_color)),
                        source_map,
                    )?
                    .0,
                secondary_color: deserializer
                    .get::<Adopt<_>>(
                        Field::new("secondary_color")
                            .or(Adopt(default.secondary_color)),
                        source_map,
                    )?
                    .0,
                success_color: deserializer
                    .get::<Adopt<_>>(
                        Field::new("success_color")
                            .or(Adopt(default.success_color)),
                        source_map,
                    )?
                    .0,
                error_color: deserializer
                    .get::<Adopt<_>>(
                        Field::new("error_color")
                            .or(Adopt(default.error_color)),
                        source_map,
                    )?
                    .0,
                text_color: deserializer
                    .get::<Adopt<_>>(
                        Field::new("text_color").or(Adopt(default.text_color)),
                        source_map,
                    )?
                    .0,
                primary_text_color: deserializer
                    .get::<Adopt<_>>(
                        Field::new("primary_text_color")
                            .or(Adopt(default.primary_text_color)),
                        source_map,
                    )?
                    .0,
                background_color: deserializer
                    .get::<Adopt<_>>(
                        Field::new("background_color")
                            .or(Adopt(default.background_color)),
                        source_map,
                    )?
                    .0,
                border_color: deserializer
                    .get::<Adopt<_>>(
                        Field::new("border_color")
                            .or(Adopt(default.border_color)),
                        source_map,
                    )?
                    .0,
                syntax: deserializer
                    .get(Field::new("syntax").or(default.syntax), source_map)?,
            };
            deserializer.done()?;
            Ok(config)
        }
    }

    impl DeserializeYaml for Syntax {
        fn expected() -> Expected {
            Expected::Mapping
        }

        fn deserialize(
            yaml: SourcedYaml,
            source_map: &SourceMap,
        ) -> yaml::Result<Self> {
            let default = Self::default();
            let mut deserializer = StructDeserializer::new(yaml)?;
            let config = Self {
                comment_color: deserializer
                    .get::<Adopt<_>>(
                        Field::new("comment_color")
                            .or(Adopt(default.comment_color)),
                        source_map,
                    )?
                    .0,
                builtin_color: deserializer
                    .get::<Adopt<_>>(
                        Field::new("builtin_color")
                            .or(Adopt(default.builtin_color)),
                        source_map,
                    )?
                    .0,
                escape_color: deserializer
                    .get::<Adopt<_>>(
                        Field::new("escape_color")
                            .or(Adopt(default.escape_color)),
                        source_map,
                    )?
                    .0,
                number_color: deserializer
                    .get::<Adopt<_>>(
                        Field::new("number_color")
                            .or(Adopt(default.number_color)),
                        source_map,
                    )?
                    .0,
                string_color: deserializer
                    .get::<Adopt<_>>(
                        Field::new("string_color")
                            .or(Adopt(default.string_color)),
                        source_map,
                    )?
                    .0,
                special_color: deserializer
                    .get::<Adopt<_>>(
                        Field::new("special_color")
                            .or(Adopt(default.special_color)),
                        source_map,
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

        fn deserialize(
            yaml: SourcedYaml,
            _source_map: &SourceMap,
        ) -> yaml::Result<Self> {
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
