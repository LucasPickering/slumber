mod error;
mod functions;

pub use error::FunctionError;
pub use functions::*;

use crate::collection::{Collection, LoadedCollection};
use anyhow::Context;
use itertools::Itertools;
use petitscript::{
    Engine, Process, Source, Value,
    ast::{Expression, FunctionCall, IntoNode, Module, ObjectLiteral},
};
use serde::de::IntoDeserializer;
use std::sync::LazyLock;
use tracing::{info, info_span};

/// Name of the PetitScript module that exposes Slumber capabilities
pub const MODULE_NAME: &str = "slumber";

/// The PetitScript engine that serves all our Petit needs. We can share one
/// engine across the entire program, and across all tests. This bad boy will
/// be configured to run any Slumber action you can throw at it.
pub static ENGINE: LazyLock<Engine> = LazyLock::new(|| {
    let _span = info_span!("Initializing PetitScript engine").entered();
    Engine::builder()
        .with_stdlib()
        .with_module(MODULE_NAME.parse().unwrap(), functions::module())
        .build()
});

/// An interface for invoking PetitScript. This is cheaply cloneable so it can
/// be shared between threads.
///
/// TODO get rid of this?
#[derive(Clone, derive_more::Debug)]
pub struct PetitEngine {}

impl PetitEngine {
    /// TODO
    pub fn new() -> Self {
        Self {}
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
            let process = ENGINE.compile(source)?;
            Self::todo2(process)
        };
        load().context(error_context)
    }

    /// TODO test only
    pub fn todo(&self, module: Module) -> anyhow::Result<LoadedCollection> {
        let process = ENGINE.compile_ast(module.s())?;
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

/// Generate a function call expression for a native function by name. Pass `R`
/// required arguments plus one keyword argument of `KW` entries. Any empty
/// kwargs will be omitted. If all kwargs are empty, omit the entire kwargs
/// object.
///
/// It would be nice to leverage static typing since we can access the Rust
/// functions, but it adds a lot of complexity that isn't worth it.
pub fn call_fn<const R: usize, const KW: usize>(
    name: &'static str,
    required: [Expression; R],
    kwargs: [(&str, Option<Expression>); KW],
) -> FunctionCall {
    let mut arguments: Vec<Expression> = required.into();
    let kwargs = kwargs
        .into_iter()
        .filter_map(|(k, v)| Some((k, v?)))
        .collect_vec();
    if !kwargs.is_empty() {
        arguments.push(ObjectLiteral::new(kwargs).into());
    }
    FunctionCall::named(name, arguments)
}

/// Generate a function call expression to the `profile()` function for a
/// particular field
pub fn profile_field(field: impl Into<String>) -> FunctionCall {
    call_fn("profile", [field.into().into()], [])
}
