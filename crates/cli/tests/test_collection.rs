//! Test the `slumber collection` subcommand

mod common;

use predicates::{prelude::predicate, str::PredicateStrExt};
use std::fs;

/// `slumber collection` prints the loaded collection in YAML
#[test]
fn test_print_config() {
    let (mut command, _) = common::slumber();
    command.args(["collection"]);
    let expected =
        serde_yaml::to_string(&common::collection_file().load().unwrap())
            .unwrap();
    command.assert().success().stdout(predicate::eq(expected));
}

/// `slumber collection --path` prints the collection path
#[test]
fn test_print_path() {
    let (mut command, _) = common::slumber();
    command.args(["collection", "--path"]);
    let expected = common::collection_file().path().display().to_string();
    command
        .assert()
        .success()
        .stdout(predicate::eq(expected).trim());
}

/// `slumber collection --edit` opens the configured editor
#[test]
fn test_edit() {
    let (mut command, _) = common::slumber();
    command.env("EDITOR", "cat").args(["collection", "--edit"]);
    // `cat` should just print out the file, which is the default content
    let expected =
        fs::read_to_string(common::collection_file().path()).unwrap();
    command.assert().success().stdout(predicate::eq(expected));
}
