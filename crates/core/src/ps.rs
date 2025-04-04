mod cereal;
mod error;
mod functions;

use crate::collection::{Collection, LoadedCollection};
use anyhow::{Context, anyhow};
use petitscript::Engine;
use serde::de::IntoDeserializer;
use std::{path::Path, sync::Arc};
use tracing::{info, info_span};

/// An interface for invoking PetitScript. This is cheaply cloneable so it can
/// be shared between threads.
#[derive(Clone, derive_more::Debug)]
pub struct PetitEngine {
    engine: Arc<petitscript::Engine>,
}

impl PetitEngine {
    /// TODO
    pub fn new() -> Self {
        let _ = info_span!("Initializing JS engine").entered();
        let mut engine = Engine::new();
        functions::register_module(&mut engine);
        Self {
            engine: engine.into(),
        }
    }

    /// Load a recipe collection from a JS file. Also return the process that
    /// it was loaded from, so we can execute further functions
    pub fn load_collection(
        &self,
        path: &Path,
    ) -> anyhow::Result<LoadedCollection> {
        info!(?path, "Loading collection file");

        let load = || {
            let process = self.engine.compile(path.to_owned())?;
            let exports = process.execute()?;
            // Default exported value should be the collection
            let value = exports
                .default
                .ok_or_else(|| anyhow!("Collection not exported TODO"))?;
            let collection: Collection =
                serde_path_to_error::deserialize(value.into_deserializer())?;

            Ok::<_, anyhow::Error>(LoadedCollection {
                process,
                collection,
            })
        };
        load()
            .with_context(|| format!("Error loading collection from {path:?}"))
    }
}

impl Default for PetitEngine {
    fn default() -> Self {
        Self::new()
    }
}
