use crate::http::RequestError;
use anyhow::Context;
use derive_more::{DerefMut, Display};
use serde::de::DeserializeOwned;
use std::{
    fmt, fs,
    iter::FusedIterator,
    ops::Deref,
    path::{Path, PathBuf},
};
use strum::{EnumCount, IntoEnumIterator};
use tracing::error;

const WEBSITE: &str = "https://slumber.lucaspickering.me";

/// Get a link to a page on the doc website. This will append the doc prefix,
/// as well as the suffix.
///
/// ```
/// assert_eq!(
///     doc_link("api/chain"),
///     "https://slumber.lucaspickering.me/book/api/chain.html",
/// );
/// ```
pub fn doc_link(path: &str) -> String {
    format!("{WEBSITE}/book/{path}.html")
}

/// Parse bytes (probably from a file) into YAML. This will merge any
/// anchors/aliases.
pub fn parse_yaml<T: DeserializeOwned>(bytes: &[u8]) -> serde_yaml::Result<T> {
    // Two-step parsing is required for anchor/alias merging
    let mut yaml_value = serde_yaml::from_slice::<serde_yaml::Value>(bytes)?;
    yaml_value.apply_merge()?;
    serde_yaml::from_value(yaml_value)
}

/// A value that can be replaced in-place. This is useful for two purposes:
/// - Transferring ownership of values from old to new
/// - Dropping the old value before creating the new one
/// This struct has one invariant: The value is always defined, *except* while
/// the replacement closure is executing. Better make sure that guy doesn't
/// panic!
#[derive(Debug)]
pub struct Replaceable<T>(Option<T>);

impl<T> Replaceable<T> {
    pub fn new(value: T) -> Self {
        Self(Some(value))
    }

    /// Replace the old value with the new one. The function that generates the
    /// new value consumes the old one.
    ///
    /// The only time this value will panic on access is while the passed
    /// closure is executing (or during unwind if it panicked).
    pub fn replace(&mut self, f: impl Fn(T) -> T) {
        let old = self.0.take().expect("Replaceable value not present!");
        self.0 = Some(f(old));
    }
}

/// Access the inner value. If mid-replacement, this will panic
impl<T> Deref for Replaceable<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref().expect("Replacement in progress or failed")
    }
}

/// Access the inner value. If mid-replacement, this will panic
impl<T> DerefMut for Replaceable<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.as_mut().expect("Replacement in progress or failed")
    }
}

/// A wrapper around `PathBuf` that makes it impossible to access a directory
/// path without creating the dir first. The idea is to prevent all the possible
/// bugs that could occur when a directory doesn't exist.
///
/// If you just want to print the path without having to create it (e.g. for
/// debug output), use the `Debug` or `Display` impls.
#[derive(Debug, Display)]
#[display("{}", _0.display())]
pub struct Directory(PathBuf);

impl Directory {
    /// Root directory for all generated files. The value is contextual:
    /// - In development, use a directory in the current directory
    /// - In release, use a platform-specific directory in the user's home
    pub fn root() -> Self {
        if cfg!(debug_assertions) {
            Self(Path::new("./data/").into())
        } else {
            // According to the docs, this dir will be present on all platforms
            // https://docs.rs/dirs/latest/dirs/fn.data_dir.html
            Self(dirs::data_dir().unwrap().join("slumber"))
        }
    }

    /// Directory to store log files
    pub fn log() -> Self {
        Self(Self::root().0.join("log"))
    }

    /// Create this directory, and return the path. This is the only way to
    /// access the path value directly, enforcing that it can't be used without
    /// being created.
    pub fn create(self) -> anyhow::Result<PathBuf> {
        fs::create_dir_all(&self.0)
            .context("Error creating directory `{self}`")?;
        Ok(self.0)
    }
}

pub trait ResultExt<T, E>: Sized {
    /// If this is an error, trace it. Return the same result.
    fn traced(self) -> Self;
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
}

impl<T> ResultExt<T, RequestError> for Result<T, RequestError> {
    fn traced(self) -> Self {
        if let Err(err) = &self {
            error!(error = %err);
        }
        self
    }
}

/// Helper to printing bytes. If the bytes aren't valid UTF-8, a message about
/// them being invalid will be printed instead.
pub struct MaybeStr<'a>(pub &'a [u8]);

impl<'a> Display for MaybeStr<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Ok(s) = std::str::from_utf8(self.0) {
            write!(f, "{s}")
        } else {
            let bytes_per_line = 12;
            // Format raw bytes in pairs of bytes
            for (i, byte) in self.0.iter().enumerate() {
                if i > 0 {
                    // Add whitespace before this group. Only use line breaks
                    // in alternate mode
                    if f.alternate() && i % bytes_per_line == 0 {
                        writeln!(f)?;
                    } else {
                        write!(f, " ")?;
                    }
                }

                write!(f, "{byte:02x}")?;
            }
            Ok(())
        }
    }
}

/// A wrapper around two iterable enums, chaining them into a single iterator
#[derive(Copy, Clone, Debug, Display, PartialEq)]
pub enum EnumChain<T, U> {
    #[display("{}", _0)]
    T(T),
    #[display("{}", _0)]
    U(U),
}

/// Only the first type needs to be defaultable. Ideally we could have a second
/// implementation with `T, U: Default` but that would conflict.
impl<T: Default, U> Default for EnumChain<T, U> {
    fn default() -> Self {
        Self::T(T::default())
    }
}

impl<T: EnumCount, U: EnumCount> EnumCount for EnumChain<T, U> {
    const COUNT: usize = T::COUNT + U::COUNT;
}

impl<T, U> IntoEnumIterator for EnumChain<T, U>
where
    T: Clone + IntoEnumIterator,
    U: Clone + IntoEnumIterator,
{
    type Iterator = EnumIterChain<T, U>;

    fn iter() -> EnumIterChain<T, U> {
        EnumIterChain::default()
    }
}

/// Iterator for [EnumChain]
#[derive(Clone)]
pub struct EnumIterChain<T: IntoEnumIterator, U: IntoEnumIterator> {
    t_iter: T::Iterator,
    u_iter: U::Iterator,
}

impl<T, U> Default for EnumIterChain<T, U>
where
    T: IntoEnumIterator,
    U: IntoEnumIterator,
{
    fn default() -> Self {
        Self {
            t_iter: T::iter(),
            u_iter: U::iter(),
        }
    }
}

impl<T, U> Iterator for EnumIterChain<T, U>
where
    T: IntoEnumIterator,
    U: IntoEnumIterator,
{
    type Item = EnumChain<T, U>;

    fn next(&mut self) -> Option<<Self as Iterator>::Item> {
        self.t_iter
            .next()
            .map(EnumChain::T)
            .or_else(|| self.u_iter.next().map(EnumChain::U))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let (t_lower, t_upper) = self.t_iter.size_hint();
        let (u_lower, u_upper) = self.u_iter.size_hint();
        (
            t_lower + u_lower,
            if let (Some(t_upper), Some(u_upper)) = (t_upper, u_upper) {
                Some(t_upper + u_upper)
            } else {
                None
            },
        )
    }
}

impl<T, U> ExactSizeIterator for EnumIterChain<T, U>
where
    T: IntoEnumIterator,
    T::Iterator: ExactSizeIterator,
    U: IntoEnumIterator,
    U::Iterator: ExactSizeIterator,
{
    fn len(&self) -> usize {
        self.t_iter.len() + self.u_iter.len()
    }
}

impl<T, U> DoubleEndedIterator for EnumIterChain<T, U>
where
    T: IntoEnumIterator,
    T::Iterator: DoubleEndedIterator,
    U: IntoEnumIterator,
    U::Iterator: DoubleEndedIterator,
{
    fn next_back(&mut self) -> Option<<Self as Iterator>::Item> {
        self.u_iter
            .next_back()
            .map(EnumChain::U)
            .or_else(|| self.t_iter.next_back().map(EnumChain::T))
    }
}

impl<T, U> FusedIterator for EnumIterChain<T, U>
where
    T: IntoEnumIterator,
    T::Iterator: FusedIterator,
    U: IntoEnumIterator,
    U::Iterator: FusedIterator,
{
}

#[cfg(test)]
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
#[cfg(test)]
pub(crate) use assert_err;

#[cfg(test)]
mod tests {
    use super::*;
    use strum::EnumIter;

    #[derive(Clone, Debug, PartialEq, EnumIter)]
    enum A {
        One,
        Two,
    }

    #[derive(Clone, Debug, PartialEq, EnumIter)]
    enum B {
        Three,
        Four,
        Five,
    }

    /// Forward iteration
    #[test]
    fn test_enum_chain_iter() {
        assert_eq!(
            <EnumChain::<A, B>>::iter().collect::<Vec<_>>(),
            vec![
                EnumChain::T(A::One),
                EnumChain::T(A::Two),
                EnumChain::U(B::Three),
                EnumChain::U(B::Four),
                EnumChain::U(B::Five),
            ]
        );
    }

    /// Backward iteration
    #[test]
    fn test_enum_chain_rev() {
        assert_eq!(
            <EnumChain::<A, B>>::iter().rev().collect::<Vec<_>>(),
            vec![
                EnumChain::U(B::Five),
                EnumChain::U(B::Four),
                EnumChain::U(B::Three),
                EnumChain::T(A::Two),
                EnumChain::T(A::One),
            ]
        );
    }

    /// Iter len
    #[test]
    fn test_enum_chain_len() {
        let mut iter = <EnumChain<A, B>>::iter();
        assert_eq!(iter.len(), 5);
        iter.next();
        assert_eq!(iter.len(), 4);
        iter.next();
        // Step into B
        iter.next();
        assert_eq!(iter.len(), 2);
        iter.next();
        iter.next();
        assert_eq!(iter.len(), 0);
    }
}
