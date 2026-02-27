//! Test user actions with side-effects

use crate::common::{Runner, TestBackend, backend, collection_file};
use rstest::rstest;
use slumber_core::{
    collection::{Collection, Recipe, RecipeId},
    http::HttpMethod,
    test_util::by_id,
};
use slumber_tui::Tui;
use slumber_util::{DataDir, Factory, data_dir};
use terminput::KeyCode;

mod common;

/// Render a body and copy it to the clipboard
#[rstest]
#[tokio::test]
async fn test_copy_body(backend: TestBackend, data_dir: DataDir) {
    // Create a collection file to be loaded
    let recipe_id: RecipeId = "textBody".into();
    let collection = Collection {
        recipes: by_id([Recipe {
            id: recipe_id.clone(),
            url: "http://localhost".into(),
            method: HttpMethod::Post,
            body: Some("{{ prompt(message='Body?') }}".into()),
            ..Recipe::factory(())
        }])
        .into(),
        ..Collection::factory(())
    };
    let collection_path = collection_file(&data_dir, &collection);
    // Rev up those fryers!!
    let tui = Tui::new(backend.clone(), Some(collection_path)).unwrap();

    let tui = Runner::new(tui)
        .send_keys([KeyCode::Char('1'), KeyCode::Right]) // Recipe > Body
        .action(&[3, 1]) // "Copy Body" action
        .wait_for_content("Body?", (12, 4).into()) // Wait for form to open
        .await
        .send_text("body!") // Fill out prompt form
        .send_key(KeyCode::Enter) // Submit
        .wait_for_content("Copied text to clipboard", (0, 19).into())
        .await
        .done()
        .await;

    assert_eq!(tui.backend().clipboard(), &["body!"]);
}
