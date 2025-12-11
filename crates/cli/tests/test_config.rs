//! Test the `slumber config` subcommand

mod common;

use predicates::{prelude::predicate, str::PredicateStrExt};
use slumber_config::Config;

/// `slumber config` prints the loaded config in YAML
#[test]
fn test_print_config() {
    let (mut command, _) = common::slumber();
    command.args(["config"]);
    // There shouldn't be a config file in the temp data dir, so we'll just see
    // the default config
    let expected = serde_yaml::to_string(&Config::default()).unwrap();
    command.assert().success().stdout(predicate::eq(expected));
}

/// `slumber config --path` prints the config path
#[test]
fn test_print_path() {
    let (mut command, data_dir) = common::slumber();
    command.args(["config", "--path"]);
    let expected = data_dir.join("config.yml").display().to_string();
    command
        .assert()
        .success()
        .stdout(predicate::eq(expected).trim());
}

/// `slumber config --edit` opens the configured editor
#[test]
fn test_edit() {
    let (mut command, _) = common::slumber();
    command.env("EDITOR", "cat").args(["config", "--edit"]);
    // `cat` should just print out the file, which is the default content
    let expected =
        String::from_utf8(Config::default_content().into_bytes()).unwrap();
    command.assert().success().stdout(predicate::eq(expected));
}
