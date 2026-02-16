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
            tui: crate::tui::deserialize_tui_config(
                &mut deserializer,
                source_map,
            )?,
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
