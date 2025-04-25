mod error;
mod functions;

pub use error::FunctionError;

use crate::collection::{Collection, LoadedCollection};
use anyhow::Context;
use petitscript::{
    Engine, Process, Source, Value,
    ast::{IntoNode, Module},
};
use serde::de::IntoDeserializer;
use std::sync::Arc;
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

    /// Load a recipe collection from a PS source. Typically the source is a
    /// path to a file, but other source types are supported for testing. Also
    /// return the process that it was loaded from, so we can execute further
    /// functions
    pub fn load_collection(
        &self,
        source: impl Source,
    ) -> anyhow::Result<LoadedCollection> {
        info!(?source, "Loading collection file");

        let error_context = format!("Error loading collection from {source:?}");
        let load = || {
            let process = self.engine.compile(source)?;
            Self::todo2(process)
        };
        load().context(error_context)
    }

    /// TODO test only
    pub fn todo(&self, module: Module) -> anyhow::Result<LoadedCollection> {
        let process = self.engine.compile_ast(module.s())?;
        Self::todo2(process)
    }

    fn todo2(process: Process) -> anyhow::Result<LoadedCollection> {
        let exports = process.execute()?;
        // Collection components (profiles, requests, etc.) should be
        // exported individually. We can treat the whole set of named
        // exports as our collection, and we'll ignore irrelevant fields
        let collection_value = Value::from(exports.named);
        let collection: Collection = serde_path_to_error::deserialize(
            collection_value.into_deserializer(),
        )?;
        Ok::<_, anyhow::Error>(LoadedCollection {
            process,
            collection,
        })
    }
}

impl Default for PetitEngine {
    fn default() -> Self {
        Self::new()
    }
}
