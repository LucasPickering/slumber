use anyhow::{anyhow, Context};
use derive_more::Display;
use std::{
    fs,
    path::{Path, PathBuf},
};

/// The root data directory. All files that Slumber creates on the system should
/// live here.
///
/// Intentionally does not implement `Debug`, to prevent accidental debug
/// printing when looking for Path's `Debug` impl, which just wraps in quotes.
#[derive(Clone, Display)]
#[display("{}", _0.display())]
pub struct DataDirectory(PathBuf);

impl DataDirectory {
    /// Root directory for all generated files. The value is contextual:
    /// - In development, use a directory from the crate root
    /// - In release, use a platform-specific directory in the user's home
    pub fn root() -> Self {
        if cfg!(debug_assertions) {
            // If env var isn't defined, this will just become ./data/
            let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("data/");
            Self(path)
        } else {
            // According to the docs, this dir will be present on all platforms
            // https://docs.rs/dirs/latest/dirs/fn.data_dir.html
            Self(dirs::data_dir().unwrap().join("slumber"))
        }
    }

    /// Path to the log file
    pub fn log() -> FileGuard {
        // Use a random new file for each session:
        // https://github.com/LucasPickering/slumber/issues/61
        Self::root().file("log/slumber.log")
    }

    /// Get the path of a file in the directory.
    pub fn file(self, path: impl AsRef<Path>) -> FileGuard {
        FileGuard(self.0.join(path))
    }
}

/// A wrapper around a path to a specific file in the data directory. The
/// purpose is to make it easy to print a path without any side effects, but
/// enforce that the path's parent directory is created before actually using
/// it. To accomplish that, this implements `Display` but does not provide any
/// other access to the inner `PathBuf`. To get the parent, use
/// [Self::create_parent].
///
/// Intentionally does not implement Debug, to prevent accidental debug printing
/// when looking for Path's debug impl, which just wraps in quotes.
#[derive(Clone, Display)]
#[display("{}", _0.display())]
pub struct FileGuard(PathBuf);

impl FileGuard {
    /// Create the parent directory and return the path to the file. This
    /// enforces that the directory exists before using the path for any
    /// operations.
    pub fn create_parent(self) -> anyhow::Result<PathBuf> {
        let parent = self
            .0
            .parent()
            .ok_or_else(|| anyhow!("Path {:?} has no parent", self.0))?;
        fs::create_dir_all(parent)
            .with_context(|| format!("Error creating directory {parent:?}"))?;
        Ok(self.0)
    }
}
