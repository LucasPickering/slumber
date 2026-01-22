//! TODO

use ratatui::backend::TestBackend;
use rstest::{fixture, rstest};
use slumber_tui::Tui;

/// TODO
#[rstest]
fn test_collection_reload(backend: TestBackend) {
    let mut tui = Tui::new(backend, None);
    todo!();
}

/// TODO comment
/// TODO make size configurable
#[fixture]
fn backend() -> TestBackend {
    TestBackend::new(10, 10)
}
