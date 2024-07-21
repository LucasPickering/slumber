use anyhow::Context;
use derive_more::Display;
use std::{
    env,
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    process,
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
    /// This will create the directory, and return an error if that fails
    pub fn init() -> anyhow::Result<()> {
        let path = if cfg!(debug_assertions) {
            // If env var isn't defined, this will just become ./data/
            Path::new(env!("CARGO_MANIFEST_DIR")).join("data/")
        } else {
            // According to the docs, this dir will be present on all platforms
            // https://docs.rs/dirs/latest/dirs/fn.data_dir.html
            dirs::data_dir().unwrap().join("slumber")
        };

        // Create the dir
        fs::create_dir_all(&path).with_context(|| {
            format!("Error creating data directory {path:?}")
        })?;

        DATA_DIRECTORY
            .set(Self(path))
            .expect("Temporary directory is already initialized");
        Ok(())
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
