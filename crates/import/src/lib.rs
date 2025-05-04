//! Import from external formats into Slumber.
//!
//! **This crate is not semver compliant**. The version is locked to the root
//! `slumber` crate version. If you choose to depend directly on this crate, you
//! do so at your own risk of breakage.

// TODO enable other formats

mod common;
mod insomnia;
mod openapi;
// mod rest;
mod legacy;

pub use insomnia::from_insomnia;
pub use openapi::from_openapi;
// pub use rest::from_rest;
pub use legacy::from_legacy;

use indexmap::IndexMap;
use petitscript::ast::{Declaration, Expression};
use slumber_core::collection::{Profile, ProfileId, RecipeTree};
use std::path::Path;

/// TODO
#[derive(Debug)]
pub struct ImportCollection {
    /// TODO
    declarations: Vec<Declaration>,
    /// TODO
    profiles: IndexMap<ProfileId, Profile<Expression>>,
    /// TODO
    recipes: RecipeTree<Expression>,
}

pub fn from_rest(_: impl AsRef<Path>) -> anyhow::Result<ImportCollection> {
    todo!()
}
