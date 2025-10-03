use crate::paths::get_repo_root;
use rstest::fixture;
use std::{
    env,
    error::Error,
    fmt::Debug,
    fs,
    ops::Deref,
    path::{Path, PathBuf},
};
use tracing::error;
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
    get_repo_root().join("test_data")
}

/// Create a new temporary folder. This will include a random subfolder to
/// guarantee uniqueness for this test.
#[fixture]
pub fn temp_dir() -> TempDir {
    TempDir::new()
}

/// Guard for a temporary directory. Create the directory on creation, delete
/// it on drop.
#[derive(Debug)]
pub struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let path = env::temp_dir().join(Uuid::new_v4().to_string());
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Deref for TempDir {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        // Clean up
        if let Err(error) = fs::remove_dir_all(&self.0) {
            error!(
                error = &error as &dyn Error,
                "Error deleting temporary directory `{}`",
                self.0.display()
            );
        }
    }
}

/// Assert a result is the `Err` variant and the stringified error contains
/// the given message. If you have an error that implements [std::error::Error],
/// you should use the function [assert_err] instead. This is only useful for
/// `anyhow` errors.
#[macro_export]
macro_rules! assert_err {
    // I'd like to get rid of this and fully replace it with the function
    // assert_err(), but that one doesn't work well with anyhow errors
    ($result:expr, $msg:expr) => {{
        let error = $result.unwrap_err();
        let msg = $msg;
        let actual = format!("{error:#}");
        assert!(
            actual.contains(msg),
            "Expected error message to contain {msg:?}, but was: {actual:?}"
        )
    }};
}

/// Assert a result is the `Err` variant and the stringified error *contains*
/// the given message. The `Err` variant type must implement `Display`. The
/// error will be formatted with its entire chain of sources, to make nested
/// errors easy to match.
#[track_caller]
pub fn assert_err<T, E>(result: Result<T, E>, expected_error: &str)
where
    T: Debug,
    E: 'static + Debug + Error + Send + Sync,
{
    let error = result.unwrap_err();
    let actual = format_error_chain(error);
    assert!(
        actual.contains(expected_error),
        "Expected error message to contain {expected_error:?}, but was: \
        {actual:?}"
    );
}

/// Assert that a result value matches the expected result. If the expectation
/// is `Ok`, then the value will be unwrapped to `Ok` and checked for equality
/// against the expected value. If the expectation is `Err`, the value will be
/// unwrapped to `Err` and checked that the error message **contains** the
/// expected `Err` string.
///
/// The error will be formatted with its entire chain of sources, to make nested
/// errors easy to match.
#[track_caller]
pub fn assert_result<TA, TE, E>(
    result: Result<TA, E>,
    expected: Result<TE, &str>,
) where
    TA: Debug + PartialEq<TE>,
    TE: Debug,
    E: 'static + Debug + Error + Send + Sync,
{
    match expected {
        Ok(expected) => {
            let value = result.unwrap();
            assert_eq!(value, expected);
        }
        Err(expected) => assert_err(result, expected),
    }
}

/// Stringify an error with all its causes
fn format_error_chain(error: impl Error) -> String {
    let mut s = error.to_string();
    let mut source = error.source();
    while let Some(error) = source {
        s.push_str(": ");
        s.push_str(&error.to_string());
        source = error.source();
    }
    s
}

/// Assert the given expression matches a pattern and optional condition.
/// Additionally, evaluate an expression using the bound pattern. This can be
/// used to apply additional assertions inline, or extract bound values to use
/// in subsequent statements.
#[macro_export]
macro_rules! assert_matches {
    ($expr:expr, $pattern:pat $(if $condition:expr)? $(,)?) => {
        $crate::assert_matches!($expr, $pattern $(if $condition)? => ());
    };
    ($expr:expr, $pattern:pat $(if $condition:expr)? => $output:expr $(,)?) => {
        match $expr {
            // If a conditional was given, check it. This has to be a separate
            // arm to prevent borrow fighting over the matched value
            $(value @ $pattern if !$condition => {
                panic!(
                    "Value {value:?} does not match condition {condition}",
                    condition = stringify!($condition),
                );
            })?
            #[expect(unused_variables)]
            $pattern => $output,
            value => panic!(
                "Unexpected value {value:?} does not match pattern {expected}",
                expected = stringify!($pattern),
            ),
        }
    };
}
