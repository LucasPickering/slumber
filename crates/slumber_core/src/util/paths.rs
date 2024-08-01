use anyhow::Context;
use derive_more::Display;
use std::{
    borrow::Cow,
    env,
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    process::{self},
    sync::OnceLock,
};

// Store directories statically so we can create them once at startup and access
// them subsequently anywhere
static DATA_DIRECTORY: OnceLock<DataDirectory> = OnceLock::new();
static TEMP_DIRECTORY: OnceLock<TempDirectory> = OnceLock::new();

/// The root data directory. All files that Slumber creates on the system should
/// live here.
#[derive(Debug, Display)]
#[display("{}", _0.display())]
pub struct DataDirectory(PathBuf);

impl DataDirectory {
    /// Initialize directory for all generated files. The path is contextual:
    /// - In development, use a directory from the crate root
    /// - In release, use a platform-specific directory in the user's home
    ///
    /// This will create the directory, and return an error if that fails
    pub fn init() -> anyhow::Result<()> {
        let path = Self::get_directory();

        // Create the dir
        fs::create_dir_all(&path).with_context(|| {
            format!("Error creating data directory {path:?}")
        })?;

        DATA_DIRECTORY
            .set(Self(path))
            .expect("Temporary directory is already initialized");
        Ok(())
    }

    #[cfg(debug_assertions)]
    fn get_directory() -> PathBuf {
        get_repo_root().join("data/")
    }

    #[cfg(not(debug_assertions))]
    fn get_directory() -> PathBuf {
        // According to the docs, this dir will be present on all platforms
        // https://docs.rs/dirs/latest/dirs/fn.data_dir.html
        dirs::data_dir().unwrap().join("slumber")
    }

    /// Get a reference to the global directory for permanent data. See
    /// [Self::init] for more info.
    pub fn get() -> &'static Self {
        DATA_DIRECTORY
            .get()
            .expect("Temporary directory is not initialized")
    }

    /// Build a path to a file in this directory
    pub fn file(&self, path: impl AsRef<Path>) -> PathBuf {
        self.0.join(path)
    }
}

/// A singleton temporary directory, which should be used to store ephemeral
/// data such as logs. This directory *is* unique to Slumber, but is *not*
/// unique to this particular process.
#[derive(Debug, Display)]
#[display("{}", path.display())]
pub struct TempDirectory {
    path: PathBuf,
    /// Absolute path to the log file
    log_file: PathBuf,
}

impl TempDirectory {
    /// Initialize the temporary directory. The directory is *not* guaranteed to
    /// be empty or unique to this process, per [std::env::temp_dir]. It
    /// *will* however include a `slumber/` suffix so it's safe to assume
    /// everything in the directory is Slumber-related. Use [Self::get] to get
    /// the created directory.
    pub fn init() -> anyhow::Result<()> {
        // Create the temp dir
        let path = env::temp_dir().join("slumber");
        fs::create_dir_all(&path).with_context(|| {
            format!("Error creating temporary directory {path:?}")
        })?;

        // Use our PID in the log file so it's unique and easy to find. It's
        // possible a PID gets re-used, so wipe out the file to be safe.
        let log_file =
            path.join(format!("slumber.{pid}.log", pid = process::id()));
        OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&log_file)
            .with_context(|| format!("Error creating log file {log_file:?}"))?;

        TEMP_DIRECTORY
            .set(Self { path, log_file })
            .expect("Temporary directory is already initialized");
        Ok(())
    }

    /// Get a reference to the global directory for temporary data. See
    /// [Self::init] for more info.
    pub fn get() -> &'static Self {
        TEMP_DIRECTORY
            .get()
            .expect("Temporary directory is not initialized")
    }

    /// Get the path to this session's log file. This will create a new file
    /// that's guaranteed to have a unique name. Each session gets its own file
    /// in the temp directory, so that multiple sessions don't intersperse their
    /// logs.
    pub fn log(&self) -> &PathBuf {
        &self.log_file
    }
}

/// Get path to the root of the git repo. This is needed because this crate
/// doesn't live at the repo root, so we can't use `CARGO_MANIFEST_DIR`. Path
/// will be cached so subsequent calls are fast. If the path can't be found,
/// fall back to the current working directory instead. Always returns an
/// absolute path.
#[cfg(any(debug_assertions, test))]
pub(crate) fn get_repo_root() -> &'static Path {
    use crate::util::ResultTraced;
    use std::process::Command;

    static CACHE: OnceLock<PathBuf> = OnceLock::new();

    CACHE.get_or_init(|| {
        let try_get = || -> anyhow::Result<PathBuf> {
            let output = Command::new("git")
                .args(["rev-parse", "--show-toplevel"])
                .output()?;
            let path = String::from_utf8(output.stdout)?;
            Ok(path.trim().into())
        };
        try_get()
            .context("Error getting repo root path")
            .traced()
            .unwrap_or_default()
    })
}

/// Expand a leading `~` in a path into the user's home directory. Only expand
/// if the `~` is the sole component, or trailed by a slash. In other words,
/// `~test.txt` will *not* be expanded. Given path will be cloned only if
pub fn expand_home<'a>(path: impl Into<Cow<'a, Path>>) -> Cow<'a, Path> {
    let path: Cow<_> = path.into();
    match path.strip_prefix("~") {
        Ok(rest) => {
            let Some(home_dir) = dirs::home_dir() else {
                return path;
            };
            home_dir.join(rest).into()
        }
        Err(_) => path,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;
    use std::path::PathBuf;

    #[rstest]
    #[case::empty("", "")]
    #[case::plain("test.txt", "test.txt")]
    #[case::tilde_only("~", "$HOME")]
    #[case::tilde_dir("~/test.txt", "$HOME/test.txt")]
    #[case::tilde_double("~/~/test.txt", "$HOME/~/test.txt")]
    #[case::tilde_in_filename("~test.txt", "~test.txt")]
    #[case::tilde_middle("text/~/test.txt", "text/~/test.txt")]
    #[case::tilde_end("text/~", "text/~")]
    fn test_expand_home(#[case] path: PathBuf, #[case] expected: String) {
        // We're assuming this dependency is correct. This provides portability,
        // so the tests pass on windows
        let home = dirs::home_dir().unwrap();
        let home = home.to_str().unwrap();
        assert!(!home.is_empty(), "Home dir is empty"); // Sanity
        let expected = expected.replace("$HOME", home);
        assert_eq!(expand_home(&path).as_ref(), PathBuf::from(expected));
    }
}
