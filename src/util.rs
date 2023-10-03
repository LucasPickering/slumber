use anyhow::anyhow;
use std::{fmt::Debug, ops::Deref};
use tracing::error;

/// A slightly spaghetti helper for finding an item in a list by value. We
/// expect the item to be there, so if it's missing return an error with a
/// friendly message for the user.
pub fn find_by<E, K: Debug + PartialEq>(
    mut vec: Vec<E>,
    extractor: impl Fn(&E) -> &K,
    target: &K,
    not_found_message: &str,
) -> anyhow::Result<E> {
    let index = vec.iter().position(|element| extractor(element) == target);
    match index {
        Some(index) => Ok(vec.swap_remove(index)),
        None => {
            let options: Vec<&K> = vec.iter().map(extractor).collect();
            Err(anyhow!(
                "{not_found_message} {target:?}; Options are: {options:?}"
            ))
        }
    }
}

pub trait ResultExt<T, E>: Sized {
    /// If this is an error, trace it. Return the same result.
    fn traced(self) -> Self;

    /// Return the value if `Ok`, or call a given function on the error if
    /// `Err`.
    fn ok_or_apply(self, op: impl FnOnce(E)) -> Option<T>;
}

// This is deliberately *not* implemented for non-anyhow errors, because we only
// want to trace errors that have full context attached
impl<T> ResultExt<T, anyhow::Error> for anyhow::Result<T> {
    fn traced(self) -> Self {
        if let Err(err) = &self {
            error!(error = err.deref());
        }
        self
    }

    fn ok_or_apply(self, op: impl FnOnce(anyhow::Error)) -> Option<T> {
        match self {
            Ok(value) => Some(value),
            Err(err) => {
                op(err);
                None
            }
        }
    }
}

#[cfg(test)]
macro_rules! assert_err {
    ($e:expr, $msg:expr) => {
        let msg = $msg;
        let error = $e.unwrap_err().to_string();
        assert!(
            error.contains(msg),
            "Expected error message to contain {msg:?}, but was: {error:?}"
        )
    };
}

#[cfg(test)]
pub(crate) use assert_err;
