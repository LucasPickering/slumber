//! General test utilities, that apply to all parts of the program

use crate::{
    collection::HasId,
    template::{Prompt, Prompter, Template},
    util::ResultExt,
};
use anyhow::Context;
use derive_more::Deref;
use indexmap::IndexMap;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use rstest::fixture;
use std::{
    env, fs,
    path::{Path, PathBuf},
};
use uuid::Uuid;

/// Test-only trait to build a placeholder instance of a struct. This is similar
/// to `Default`, but allows for useful placeholders that may not make sense in
/// the context of the broader app. It also makes it possible to implement a
/// factory for a type that already has `Default`.
///
/// Factories can also be parameterized, meaning the implementor can define
/// convenient knobs to let the caller customize the generated type. Each type
/// can have any number of `Factory` implementations, so you can support
/// multiple param types.
pub trait Factory<Param = ()> {
    fn factory(param: Param) -> Self;
}

/// Directory containing static test data
#[fixture]
pub fn test_data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("test_data")
}

/// Create a new temporary folder. This will include a random subfolder to
/// guarantee uniqueness for this test.
#[fixture]
pub fn temp_dir() -> TempDir {
    TempDir::new()
}

/// Guard for a temporary directory. Create the directory on creation, delete
/// it on drop.
#[derive(Debug, Deref)]
pub struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let path = env::temp_dir().join(Uuid::new_v4().to_string());
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        // Clean up
        let _ = fs::remove_dir_all(&self.0)
            .with_context(|| {
                format!("Error deleting temporary directory {:?}", self.0)
            })
            .traced();
    }
}

/// Return a static value when prompted, or no value if none is given
#[derive(Debug, Default)]
pub struct TestPrompter {
    value: Option<String>,
}

impl TestPrompter {
    pub fn new<T: Into<String>>(value: Option<T>) -> Self {
        Self {
            value: value.map(Into::into),
        }
    }
}

impl Prompter for TestPrompter {
    fn prompt(&self, prompt: Prompt) {
        // If no value was given, check default. If no default, don't respond
        if let Some(value) = self.value.as_ref() {
            prompt.channel.respond(value.clone())
        } else if let Some(default) = prompt.default {
            prompt.channel.respond(default);
        }
    }
}

impl From<&str> for Template {
    fn from(value: &str) -> Self {
        value.to_owned().try_into().unwrap()
    }
}
// Can't implement this for From<String> because it conflicts with TryFrom

/// Construct a map of values keyed by their ID
pub fn by_id<T: HasId>(
    values: impl IntoIterator<Item = T>,
) -> IndexMap<T::Id, T> {
    values
        .into_iter()
        .map(|value| (value.id().clone(), value))
        .collect()
}

/// Helper for creating a header map
pub fn header_map<'a>(
    headers: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> HeaderMap {
    headers
        .into_iter()
        .map(|(header, value)| {
            (
                HeaderName::try_from(header).unwrap(),
                HeaderValue::try_from(value).unwrap(),
            )
        })
        .collect()
}

/// Assert a result is the `Err` variant, and the stringified error contains
/// the given message
macro_rules! assert_err {
    ($e:expr, $msg:expr) => {{
        use itertools::Itertools as _;

        let msg = $msg;
        // Include all source errors so wrappers don't hide the important stuff
        let error: anyhow::Error = $e.unwrap_err().into();
        let actual = error.chain().map(ToString::to_string).join(": ");
        assert!(
            actual.contains(msg),
            "Expected error message to contain {msg:?}, but was: {actual:?}"
        )
    }};
}
pub(crate) use assert_err;

/// Assert the given expression matches a pattern. Optionally extract bound
/// values from the pattern using the `=>` syntax.
macro_rules! assert_matches {
    ($expr:expr, $pattern:pat $(,)?) => {
        crate::test_util::assert_matches!($expr, $pattern => ());
    };
    ($expr:expr, $pattern:pat => $bindings:expr $(,)?) => {
        match $expr {
            $pattern => $bindings,
            value => panic!(
                "Unexpected value; \
                {value:?} does not match expected {expected}",
                expected = stringify!($pattern)
            ),
        }
    };
}
pub(crate) use assert_matches;
