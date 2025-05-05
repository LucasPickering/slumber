#![forbid(unsafe_code)]
#![deny(clippy::all)]
#![allow(async_fn_in_trait)]

//! Core frontend-agnostic functionality for Slumber, agnostic of the front end.
//!
//! **This crate is not semver compliant**. The version is locked to the root
//! `slumber` crate version. If you choose to depend directly on this crate, you
//! do so at your own risk of breakage.

// TODO export directly from some of these modules

pub mod collection;
pub mod database;
pub mod http;
pub mod petit;
pub mod render;
#[cfg(any(test, feature = "test"))]
pub mod test_util;
pub mod util;
