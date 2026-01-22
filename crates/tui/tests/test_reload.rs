//! Test collection reloading and error states

mod common;

use crate::common::{Runner, backend};
use ratatui::backend::TestBackend;
use rstest::rstest;
use slumber_tui::Tui;
use slumber_util::{paths::DATA_DIRECTORY_ENV_VARIABLE, temp_dir};
use std::path::{Path, PathBuf};
use tokio::fs;

/// Load a collection, then change the file to trigger a reload
#[rstest]
#[tokio::test]
async fn test_collection_reload(backend: TestBackend) {
    // Every test needs its own data dir, for isolation. This effectively
    // single-threads the tests. We could fix this
    let data_dir = temp_dir();
    let _env_guard = env_lock::lock_env([(
        DATA_DIRECTORY_ENV_VARIABLE,
        Some(data_dir.to_str().unwrap()),
    )]);
    // Start with an empty collection
    let collection_path = collection_file(&data_dir).await;
    let tui = Tui::new(backend, Some(collection_path.clone())).unwrap();
    let db = tui.database().clone();

    // Make sure the initial load is correct
    let collection = tui.collection().expect("Collection should be loaded");
    assert_eq!(collection.recipes.iter().count(), 0);
    // Collection name should be set in the DB
    assert_eq!(db.metadata().unwrap().name.as_deref(), Some("Test"));

    let tui = Runner::run(tui)
        // Update the file
        .run_until(fs::write(
            &collection_path,
            r#"
name: Test Reloaded

requests:
    test:
        method: "GET"
        url: "test"
"#,
        ))
        .await
        // Wait for it to be picked up by the TUI
        .wait_for(|| {
            db.metadata().unwrap().name.as_deref() == Some("Test Reloaded")
        })
        .await
        .done()
        .await;

    // And it's done!
    let collection = tui.collection().expect("Collection should be loaded");
    assert_eq!(collection.recipes.iter().count(), 1);
    // Name was updated too
    assert_eq!(
        db.metadata().unwrap().name.as_deref(),
        Some("Test Reloaded")
    );
}

/// Create an empty collection file and return its path
async fn collection_file(directory: &Path) -> PathBuf {
    let path = directory.join("slumber.yml");
    fs::write(&path, "name: Test").await.unwrap();
    path
}
