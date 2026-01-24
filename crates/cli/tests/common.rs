#![allow(unused)]

use assert_cmd::{Command, cargo::cargo_bin_cmd};
use slumber_core::collection::CollectionFile;
use slumber_util::{TempDir, temp_dir};
use std::{
    env,
    ops::Deref,
    path::{Path, PathBuf},
};

/// Get a command to run Slumber. This will also return the data directory that
/// will be used for the database. Most tests can just ignore this.
pub fn slumber() -> (Command, TempDir) {
    let data_dir = temp_dir();
    let mut command = cargo_bin_cmd!("slumber_cli");
    command
        .current_dir(tests_dir())
        .args(["--data-dir", data_dir.deref().to_str().unwrap()]);
    (command, data_dir)
}

pub fn tests_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests")
}

/// Path to the CLI test collection file
pub fn collection_file() -> CollectionFile {
    CollectionFile::new(Some(tests_dir().join("slumber.yml"))).unwrap()
}
