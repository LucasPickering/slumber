//! Test collection loading, reloading, and error states

mod common;

use crate::common::{Runner, TestBackend, backend};
use rstest::rstest;
use slumber_core::{
    collection::{Collection, CollectionFile, Recipe},
    http::HttpMethod,
    test_util::by_id,
};
use slumber_tui::Tui;
use slumber_util::{DataDir, Factory, data_dir};
use std::path::{Path, PathBuf};
use terminput::KeyCode;
use tokio::fs;

/// Load a collection, then change the file to trigger a reload
#[rstest]
#[tokio::test]
async fn test_collection_reload(backend: TestBackend, data_dir: DataDir) {
    // Start with an empty collection
    let collection_path = collection_file(&data_dir, "name: Test").await;
    let tui = Tui::new(backend.clone(), Some(collection_path.clone())).unwrap();

    // Make sure the initial load is correct
    let collection = tui.collection().expect("Collection should be loaded");
    assert_eq!(collection.recipes.iter().count(), 0);
    // Collection name should be set in the DB
    assert_eq!(
        tui.database().metadata().unwrap().name.as_deref(),
        Some("Test")
    );

    let tui = Runner::new(tui)
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
        .wait_for_content("GET test", (1, 3).into())
        .await
        .done()
        .await;

    // And it's done!
    let collection = tui.collection().expect("Collection should be loaded");
    assert_eq!(collection.recipes.iter().count(), 1);
    // Name was updated too
    assert_eq!(
        tui.database().metadata().unwrap().name.as_deref(),
        Some("Test Reloaded")
    );

    // Now test swapping out the file. Emulates how vim/helix save
    // https://github.com/LucasPickering/slumber/issues/706
    let temp_file = data_dir.join("tmp.yml");
    fs::write(&temp_file, "name: Test Swapped").await.unwrap();

    let tui = Runner::new(tui)
        .run_until(fs::rename(&temp_file, &collection_path))
        .await
        .wait_for_content("No recipes defined", (1, 3).into())
        .await
        .done()
        .await;

    assert_eq!(
        tui.database().metadata().unwrap().name.as_deref(),
        Some("Test Swapped")
    );
}

/// Test an error in the collection during initial load. Should shove us
/// into an error state. After fixing the error, it will reload with the
/// valid collection.
#[rstest]
#[tokio::test]
async fn test_initial_load_error(backend: TestBackend, data_dir: DataDir) {
    // Start with an invalid collection
    let collection_path = collection_file(&data_dir, "requests: 3").await;

    let tui = Tui::new(backend, Some(collection_path.clone())).unwrap();

    // Should load into an error state - no collection present
    let tui = Runner::new(tui).done().await; // Draw so we can check output
    assert_eq!(tui.collection(), None);
    tui.backend().assert_buffer_contains(
        "Expected mapping, received `3`",
        (2, 4).into(),
    );

    // Update the file, make sure it's reflected
    let tui = Runner::new(tui)
        .run_until(fs::write(&collection_path, "requests: {}"))
        .await
        .wait_for_content("No recipes defined", (1, 3).into())
        .await
        .done()
        .await;

    // And it's done!
    assert_eq!(tui.collection(), Some(&Collection::default()));
}

/// Collection is loaded successfully on startup, but then changed to have
/// an error. The old collection should remain in use but the error is
/// shown.
#[rstest]
#[tokio::test]
async fn test_reload_error(backend: TestBackend, data_dir: DataDir) {
    // Start with an empty collection
    let collection_path = collection_file(&data_dir, "").await;
    let tui = Tui::new(backend, Some(collection_path.clone())).unwrap();

    // Make sure it loaded correctly
    let tui = Runner::new(tui).done().await; // Draw so we can check output
    assert_eq!(tui.collection(), Some(&Collection::default()));
    tui.backend()
        .assert_buffer_contains("No recipes defined", (1, 3).into());

    // Update the file with an invalid collection. The error is shown but we
    // keep the old collection in use
    let tui = Runner::new(tui)
        .run_until(fs::write(&collection_path, "requests: 3"))
        .await
        // Error is shown in a modal
        .wait_for_content("Expected mapping", (12, 9).into())
        .await
        .done()
        .await;

    // We remain in valid mode with the original collection
    assert_eq!(tui.collection(), Some(&Collection::default()));
    tui.backend()
        .assert_buffer_contains("No recipes defined", (1, 3).into());
}

/// Switch the selected request, which should rebuild the state entirely
#[rstest]
#[tokio::test]
async fn test_collection_switch(backend: TestBackend, data_dir: DataDir) {
    // Start with an empty collection
    let collection_path = collection_file(&data_dir, "name: Coll 1").await;
    let tui = Tui::new(backend, Some(collection_path.clone())).unwrap();

    // Create a second collection
    let other_collection_path = data_dir.join("other_slumber.yml");
    fs::write(
        &other_collection_path,
        r#"name: Coll 2
requests: {"r1": {"method": "GET", "url": "http://localhost"}}"#,
    )
    .await
    .unwrap();
    let db = tui.database().root();
    let other_collection_file =
        CollectionFile::new(Some(other_collection_path)).unwrap();
    db.clone().into_collection(&other_collection_file).unwrap();
    assert_eq!(db.get_collections().unwrap().len(), 2);

    // Make sure it loaded correctly
    let tui = Runner::new(tui).done().await; // Draw so we can check output
    assert_eq!(
        tui.collection(),
        Some(&Collection {
            name: Some("Coll 1".into()),
            ..Collection::default()
        })
    );
    tui.backend()
        .assert_buffer_contains("No recipes defined", (1, 3).into());

    // Open the collection menu and select the other collection
    let tui = Runner::new(tui)
        .send_keys([KeyCode::F(3), KeyCode::Down, KeyCode::Enter])
        .done()
        .await;

    assert_eq!(
        tui.collection(),
        Some(&Collection {
            name: Some("Coll 2".into()),
            recipes: by_id([Recipe {
                id: "r1".into(),
                method: HttpMethod::Get,
                url: "http://localhost".into(),
                ..Recipe::factory(())
            }])
            .into(),
            ..Default::default()
        })
    );
    tui.backend()
        .assert_buffer_contains("GET http://localhost", (1, 3).into());
}

/// Create an empty collection file and return its path
async fn collection_file(directory: &Path, content: &str) -> PathBuf {
    let path = directory.join("slumber.yml");
    fs::write(&path, content).await.unwrap();
    path
}
