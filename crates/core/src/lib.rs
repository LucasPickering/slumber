//! Core frontend-agnostic functionality for Slumber, agnostic of the front end.
//!
//! **This crate is not semver compliant**. The version is locked to the root
//! `slumber` crate version. If you choose to depend directly on this crate, you
//! do so at your own risk of breakage.

pub mod collection;
pub mod database;
pub mod http;
pub mod render;
#[cfg(any(test, feature = "test"))]
pub mod test_util;
pub mod util;

#[cfg(target_arch = "wasm32")]
pub fn main() -> String {
    use collection::CollectionFile;
    let collection = CollectionFile::new(None).unwrap().load().unwrap();
    format!("{collection:?}")
}
