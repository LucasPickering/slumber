//! Test the `slumber generate` subcommand

mod common;

/// Test generating a curl command with:
/// - URL
/// - Query params
/// - Headers
#[test]
fn test_generate_curl() {
    let (mut command, _) = common::slumber();
    command.args(["generate", "curl", "getUser"]);
    command
        .assert()
        .success()
        .stdout("curl -XGET --url 'http://server/users/username1'\n");
}

/// Make sure the profile option is reflected correctly
#[test]
fn test_generate_curl_profile() {
    let (mut command, _) = common::slumber();
    command.args(["generate", "curl", "getUser", "-p", "profile2"]);
    command
        .assert()
        .success()
        .stdout("curl -XGET --url 'http://server/users/username2'\n");
}

/// Make sure field overrides are applied correctly
#[test]
fn test_generate_curl_override() {
    let (mut command, _) = common::slumber();
    command.args(["generate", "curl", "getUser", "-o", "username=username3"]);
    command
        .assert()
        .success()
        .stdout("curl -XGET --url 'http://server/users/username3'\n");
}

// More detailed test cases for curl are defined in unit tests
