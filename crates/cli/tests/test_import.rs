#![cfg(feature = "import")]

mod common;

use rstest::rstest;
use slumber_core::collection::Collection;
use slumber_util::test_data_dir;
use std::path::{Path, PathBuf};
use tokio::fs;
use wiremock::{Mock, MockServer, ResponseTemplate, matchers};

const OPENAPI_FILE: &str = "openapi_v3_0_petstore.yml";
const OPENAPI_IMPORTED_FILE: &str = "openapi_v3_0_petstore_imported.yml";

/// Test `slumber import` from a local file to stdout
#[rstest]
fn test_import_local(test_data_dir: PathBuf) {
    let input_path = path_to_string(test_data_dir.join(OPENAPI_FILE));

    let (mut command, _) = common::slumber();
    let output = command
        .args(["import", "openapi", &input_path])
        .assert()
        .success();

    assert_openapi_imported(
        &test_data_dir,
        std::str::from_utf8(&output.get_output().stdout).unwrap(),
    );
}

/// Test `slumber import` from stdin to stdout
#[rstest]
#[tokio::test]
async fn test_import_stdin(test_data_dir: PathBuf) {
    let (mut command, _) = common::slumber();
    let output = command
        .args(["import", "openapi", "-"])
        .pipe_stdin(test_data_dir.join(OPENAPI_FILE))
        .unwrap()
        .assert()
        .success();

    assert_openapi_imported(
        &test_data_dir,
        std::str::from_utf8(&output.get_output().stdout).unwrap(),
    );
}

/// Test `slumber import` from a remote file over HTTP
#[rstest]
#[tokio::test]
async fn test_import_remote(test_data_dir: PathBuf) {
    // Mock HTTP response with the OpenAPI file content
    let server = MockServer::start().await;
    let host = server.uri();
    let openapi = fs::read_to_string(test_data_dir.join(OPENAPI_FILE))
        .await
        .unwrap();
    Mock::given(matchers::method("GET"))
        .and(matchers::path("/openapi.yml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(openapi))
        .mount(&server)
        .await;
    let url = format!("{host}/openapi.yml");

    let (mut command, _) = common::slumber();
    let output = command.args(["import", "openapi", &url]).assert().success();

    assert_openapi_imported(
        &test_data_dir,
        std::str::from_utf8(&output.get_output().stdout).unwrap(),
    );
}

/// Test `slumber import` writing output to a file
#[rstest]
#[tokio::test]
async fn test_import_to_file(test_data_dir: PathBuf) {
    let (mut command, data_dir) = common::slumber();
    let input_path = path_to_string(test_data_dir.join(OPENAPI_FILE));
    let output_path = path_to_string(data_dir.join("openapi.yml"));

    command
        .args(["import", "openapi", &input_path, &output_path])
        .assert()
        .success();

    let output = fs::read_to_string(&output_path).await.unwrap();
    assert_openapi_imported(&test_data_dir, &output);
}

/// Assert that an imported collection matches the expected value. This will
/// parse the output back into a collection and compare it to the expected.
/// We *could* just compare the raw YAML to the expected, but that makes
/// it dependent on formatting which is a lot more fragile
fn assert_openapi_imported(test_data_dir: &Path, actual: &str) {
    let actual = Collection::parse(actual).unwrap();
    let expected =
        Collection::load(&test_data_dir.join(OPENAPI_IMPORTED_FILE)).unwrap();
    assert_eq!(actual, expected);
}

fn path_to_string(path: PathBuf) -> String {
    path.into_os_string().into_string().unwrap()
}
