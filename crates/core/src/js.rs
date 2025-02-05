mod cereal;
mod error;
mod functions;

use crate::collection::Collection;
use anyhow::{anyhow, Context as _};
use petit_js::{Engine, Process};
use std::path::Path;
use tracing::{info, info_span};

/// TODO
#[derive(Clone, derive_more::Debug)]
pub struct JsEngine {
    engine: petit_js::Engine,
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
    pub async fn load_collection(
        &mut self,
        path: &Path,
    ) -> anyhow::Result<(Process, Collection)> {
        info!(?path, "Loading collection file");
        async {
            let mut process = self.engine.load(path)?;
            process.execute().await?;
            // Default exported value should be the collection
            let collection: Collection = process
                .exports()
                .default
                .ok_or_else(|| anyhow!("Collection not exported TODO"))?
                .deserialize()?;

            Ok::<_, anyhow::Error>((process, collection))
        }
        .await
        .with_context(|| format!("Error loading collection from {path:?}"))
    }
}

impl Default for JsEngine {
    fn default() -> Self {
        Self::new()
    }
}
