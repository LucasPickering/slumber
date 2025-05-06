//! PetitScript integration

mod error;
mod functions;

pub use error::FunctionError;
pub use functions::*;

use crate::collection::Collection;
use anyhow::Context;
use itertools::Itertools;
use petitscript::{
    Engine, Process, Source, Value,
    ast::{Expression, FunctionCall, ObjectLiteral, TemplateChunk},
};
use serde::de::IntoDeserializer;
use std::sync::LazyLock;
use tracing::{info, info_span};

/// Name of the PetitScript module that exposes Slumber capabilities
pub const MODULE_NAME: &str = "slumber";

/// The PetitScript engine that serves all our Petit needs. We can share one
/// engine across the entire program and all tests. This bad boy will be
/// configured to run any Slumber action you can throw at it.
pub static ENGINE: LazyLock<Engine> = LazyLock::new(|| {
    let _span = info_span!("Initializing PetitScript engine").entered();
    Engine::builder()
        .with_stdlib()
        .with_module(MODULE_NAME.parse().unwrap(), functions::module())
        .build()
});

/// Load a recipe collection from a PS source. Typically the source is a
/// path to a file, but other source types are supported for testing. The source
/// will be compiled and executed, and the exported values are expected to
/// contain the fields of the collection. In addition to returning the
/// deserialized collection, this will also return the process from which it
/// was loaded so that we can execute further functions.
pub fn load_collection(
    source: impl Source,
) -> anyhow::Result<(Collection, Process)> {
    info!(?source, "Loading collection file");

    let error_context = format!("Error loading collection from {source:?}");
    let load = || {
        let process = ENGINE.compile(source)?;
        let exports = process.execute()?;
        // Collection components (profiles, requests, etc.) should be
        // exported individually. We can treat the whole set of named
        // exports as our collection, and we'll ignore irrelevant fields
        let collection_value = Value::from(exports.named);
        let collection: Collection = serde_path_to_error::deserialize(
            collection_value.into_deserializer(),
        )?;
        Ok::<_, anyhow::Error>((collection, process))
    };
    load().context(error_context)
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

/// Generate a template chunk expression with a call to `profile()`
pub fn profile_chunk(field: impl Into<String>) -> TemplateChunk {
    TemplateChunk::expression(profile_field(field).into())
}
