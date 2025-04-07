//! Import from external formats into Slumber.
//!
//! **This crate is not semver compliant**. The version is locked to the root
//! `slumber` crate version. If you choose to depend directly on this crate, you
//! do so at your own risk of breakage.

mod insomnia;
mod openapi;
mod rest;

pub use insomnia::from_insomnia;
pub use openapi::from_openapi;
pub use rest::from_rest;
