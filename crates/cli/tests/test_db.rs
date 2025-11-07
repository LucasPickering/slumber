//! Test `slumber db`

mod common;

use predicates::{prelude::predicate, str::PredicateStrExt};

/// `slumber db --path` prints the DB path
#[test]
fn test_print_path() {
    let (mut command, data_dir) = common::slumber();
    command.args(["db", "--path"]);
    let expected = data_dir.join("state.sqlite").display().to_string();
    command
        .assert()
        .success()
        .stdout(predicate::eq(expected).trim());
}
