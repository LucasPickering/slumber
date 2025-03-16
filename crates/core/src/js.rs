mod cereal;
mod error;
mod functions;

use crate::collection::{Collection, LoadedCollection};
use anyhow::{Context, anyhow};
use petitscript::Engine;
use serde::de::IntoDeserializer;
use std::{future::Future, path::PathBuf};
use tokio::task;
use tracing::{info, info_span};

/// TODO
#[derive(Clone, derive_more::Debug)]
pub struct JsEngine {
    engine: petitscript::Engine,
}

impl JsEngine {
    /// TODO
    pub fn new() -> Self {
        let _ = info_span!("Initializing JS engine").entered();
        let mut engine = Engine::new();
        functions::register_all(&mut engine);
        Self { engine }
    }

    /// Load a recipe collection from a JS file. Also return the process that
    /// it was loaded from, so we can execute further functions
    pub fn load_collection(
        &self,
        path: PathBuf,
    ) -> impl 'static + Future<Output = anyhow::Result<LoadedCollection>> {
        info!(?path, "Loading collection file");
        // Parse the file outside the thread so we can drop the engine ref
        let process_result = self.engine.compile(path.clone());

        async move {
            task::spawn_blocking(move || {
                let process = process_result?;
                let exports = process.execute()?;
                // Default exported value should be the collection
                let value = exports
                    .default
                    .ok_or_else(|| anyhow!("Collection not exported TODO"))?;
                let collection: Collection = serde_path_to_error::deserialize(
                    value.into_deserializer(),
                )?;

                Ok::<_, anyhow::Error>(LoadedCollection {
                    process,
                    collection,
                })
            })
            .await
            .context("TODO")?
            .with_context(|| format!("Error loading collection from {path:?}"))
        }
    }
}

impl Default for JsEngine {
    fn default() -> Self {
        Self::new()
    }
}
