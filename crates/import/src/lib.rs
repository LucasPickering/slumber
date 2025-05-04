//! Import from external formats into Slumber.
//!
//! **This crate is not semver compliant**. The version is locked to the root
//! `slumber` crate version. If you choose to depend directly on this crate, you
//! do so at your own risk of breakage.

mod common;
mod insomnia;
mod legacy;
mod openapi;
mod rest;

pub use insomnia::from_insomnia;
pub use legacy::from_legacy;
pub use openapi::from_openapi;
pub use rest::from_rest;

use indexmap::IndexMap;
use petitscript::ast::{Declaration, Expression};
use slumber_core::collection::{Profile, ProfileId, RecipeTree};

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
