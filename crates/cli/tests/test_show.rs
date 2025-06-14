//! Test the `slumber show` subcommand and its sub-subcommands: paths, config,
//! collection

mod common;

use predicates::{prelude::predicate, str::PredicateStrExt};
use rstest::rstest;
use slumber_config::Config;
use std::path::PathBuf;

/// Test `slumber show paths` prints all relevant paths
#[test]
fn test_show_paths_all() {
    let (mut command, data_dir) = common::slumber();
    command.args(["show", "paths"]);
    command.assert().success().stdout(format!(
        "Config: {config}
Database: {database}
Log file: {log_file}
Collection: {collection}
",
        config = data_dir.join("config.yml").display(),
        database = data_dir.join("state.sqlite").display(),
        log_file = data_dir.join("slumber.log").display(),
        collection = common::collection_file(),
    ));
}

/// Parameterized test for `slumber show paths <target>` for each target
#[rstest]
#[case::config("config", "config.yml")]
#[case::collection("collection", common::collection_file().path().to_owned())]
#[case::database("db", "state.sqlite")]
#[case::log("log", "slumber.log")]
fn test_show_paths_targets(#[case] target: &str, #[case] expected: PathBuf) {
    let (mut command, data_dir) = common::slumber();
    command.args(["show", "paths", target]);
    // If the expected path is absolute (in the case of collection file), the
    // join won't do anything. Most of them are relative to the data dir
    let expected = format!("{}\n", data_dir.join(expected).display());
    command.assert().stdout(expected);
}

/// Test `slumber show config` prints the loaded config in YAML
#[test]
fn test_show_config() {
    let (mut command, _) = common::slumber();
    command.args(["show", "config"]);
    // There shouldn't be a config file in the temp data dir, so we'll just see
    // the default config
    let expected = serde_yaml::to_string(&Config::default()).unwrap();
    command
        .assert()
        .success()
        .stdout(predicate::eq(expected.trim()).trim());
}

/// Test `slumber show collection` prints the loaded collection in YAML
#[test]
fn test_show_collection() {
    let (mut command, _) = common::slumber();
    command.args(["show", "collection"]);
    let expected =
        serde_yaml::to_string(&common::collection_file().load().unwrap())
            .unwrap();
    command
        .assert()
        .success()
        .stdout(predicate::eq(expected.trim()).trim());
}
