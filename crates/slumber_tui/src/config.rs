use crate::{
    input::{Action, InputBinding},
    view::Theme,
};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use slumber_core::config::{self, Config};

/// Extension of [Config], with additional TUI-specific fields. We can't put
/// this whole thing in the core crate because the TUI fields use types from
/// TUI-specific dependencies that I really don't want to put in the core crate.
#[derive(Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TuiConfig {
    /// Base config
    #[serde(flatten)]
    pub core: Config,
    /// Should templates be rendered inline in the UI, or should we show the
    /// raw text?
    pub preview_templates: bool,
    /// Overrides for default key bindings
    pub input_bindings: IndexMap<Action, InputBinding>,
    /// Visual configuration for the TUI (e.g. colors)
    pub theme: Theme,
}

impl TuiConfig {
    /// Load config from the file
    pub fn load() -> anyhow::Result<Self> {
        config::load::<Self>()
    }
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            core: Default::default(),
            preview_templates: true,
            input_bindings: Default::default(),
            theme: Default::default(),
        }
    }
}
