use path_clean::PathClean;
use std::{
    borrow::Cow,
    fs, io,
    path::{Path, PathBuf},
    sync::OnceLock,
};

#[cfg(debug_assertions)]
thread_local! {
    /// This is dev-only so it can be used in integration tests. In the past
    /// this used an env var, but it's now a thread local so integration tests
    /// can run in parallel on separate threads (env vars are process-wide).
    /// This will *not* be automatically reset, so any test that cares about the
    /// data dir needs to set this itself.
    static DATA_DIRECTORY_OVERRIDE: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

/// Lock for the log file path. A random file name is generated once during
/// startup, then used for that session
static LOG_FILE: OnceLock<PathBuf> = OnceLock::new();

/// Override the data directory **for the current thread**
///
/// This should be used via the [data_dir](super::test_util::data_dir)
#[cfg(any(debug_assertions, test, feature = "test"))]
pub fn set_data_directory(path: PathBuf) {
    DATA_DIRECTORY_OVERRIDE.with_borrow_mut(|dir| *dir = Some(path));
}

/// Reset the data directory override **for the current thread**
///
/// This is called automatically by the [data_dir](super::test_util::data_dir)
/// fixture at the end of the test
#[cfg(any(debug_assertions, test, feature = "test"))]
pub fn reset_data_directory() {
    DATA_DIRECTORY_OVERRIDE.with_borrow_mut(|dir| *dir = None);
}

/// Get the path of the directory to contain the config file (e.g. the
/// database). **Directory may not exist yet**, caller must create it.
pub fn config_directory() -> PathBuf {
    // Config dir will be present on all platforms
    // https://docs.rs/dirs/5.0.1/dirs/fn.config_dir.html
    debug_or(dirs::config_dir().unwrap().join("slumber"))
}

/// Get the path of the directory to data files (e.g. the database). **Directory
/// may not exist yet**, caller must create it.
pub fn data_directory() -> PathBuf {
    // Data dir will be present on all platforms
    // https://docs.rs/dirs/latest/dirs/fn.data_dir.html
    debug_or(dirs::data_dir().unwrap().join("slumber"))
}

/// Get the path to the log file. Each session gets a unique file within a
/// temporary directory. The parent directory **may not exist yet.** Caller must
/// ensure it is created.
pub fn log_file() -> PathBuf {
    LOG_FILE
        .get_or_init(|| {
            // Use a static file in dev for easier access
            #[cfg(debug_assertions)]
            {
                data_directory().join("slumber.log")
            }
            #[cfg(not(debug_assertions))]
            {
                use std::env;
                use uuid::Uuid;

                let directory = env::temp_dir();
                // Temp dir isn't guaranteed to be unique, so make sure the file
                // name is
                let file_name = format!("slumber-{}.log", Uuid::new_v4());
                directory.join(file_name)
            }
        })
        .clone()
}

/// In debug mode, use a local directory for all files. In release, use the
/// given path.
fn debug_or(path: PathBuf) -> PathBuf {
    #[cfg(debug_assertions)]
    {
        let _ = path; // Remove unused warning
        // Check the thread-local override first for tests
        DATA_DIRECTORY_OVERRIDE
            .with_borrow(Clone::clone)
            .unwrap_or_else(|| get_repo_root().join("data/"))
    }
    #[cfg(not(debug_assertions))]
    {
        path
    }
}

/// Ensure the parent directory of a file path exists
pub fn create_parent(path: &Path) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "Cannot create directory for path {path}; it has no parent",
                path = path.display()
            ),
        )
    })?;
    fs::create_dir_all(parent)
}

/// Get path to the root of the git repo. This is needed because this crate
/// doesn't live at the repo root, so we can't use `CARGO_MANIFEST_DIR`. Path
/// will be cached so subsequent calls are fast. If the path can't be found,
/// panic. This is only used in debug builds so it should always be in a git
/// repo.
#[cfg(any(debug_assertions, test))]
pub fn get_repo_root() -> &'static Path {
    use std::{process::Command, sync::OnceLock};

    static CACHE: OnceLock<PathBuf> = OnceLock::new();

    CACHE.get_or_init(|| {
        let output = Command::new("git")
            .args(["rev-parse", "--show-toplevel"])
            .output()
            .unwrap();
        let path = String::from_utf8(output.stdout).unwrap();
        path.trim().into()
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

/// Normalize a referenced file path, ensuring it is absolute and cannot have
/// any equivalent aliases (barring the existence of symlinks). This will:
/// - Make the path absolute by joining it with the given base path. If it's
///   already absolute, this will have no effect
/// - Expand a leading `~` to the home directory
/// - "Clean" the path by resolving `.` and `..` segments
///
/// This will *not* touch the filesystem in any way and therefore is infallible.
pub fn normalize_path(base_dir: &Path, file: &Path) -> PathBuf {
    base_dir.join(expand_home(file)).clean()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;
    use std::path::PathBuf;

    #[rstest]
    #[case::empty("", "")]
    #[case::plain("test.txt", "test.txt")]
    #[case::tilde_only("~", "{HOME}")]
    #[case::tilde_dir("~/test.txt", "{HOME}/test.txt")]
    #[case::tilde_double("~/~/test.txt", "{HOME}/~/test.txt")]
    #[case::tilde_in_filename("~test.txt", "~test.txt")]
    #[case::tilde_middle("text/~/test.txt", "text/~/test.txt")]
    #[case::tilde_end("text/~", "text/~")]
    fn test_expand_home(#[case] path: PathBuf, #[case] expected: &str) {
        let expected = replace_home(expected);
        assert_eq!(expand_home(&path).as_ref(), PathBuf::from(expected));
    }

    #[rstest]
    #[case::relative("./file.yml", "/base/file.yml")]
    #[case::absolute("./file.yml", "/base/file.yml")]
    #[case::dots("../other/./file.yml", "/other/file.yml")]
    #[case::home("./file.yml", "/base/file.yml")]
    #[case::home("~/file.yml", "{HOME}/file.yml")]
    fn test_normalize_path(#[case] file: &str, #[case] expected: &str) {
        let expected = replace_home(expected);
        assert_eq!(
            normalize_path(Path::new("/base"), Path::new(file)),
            Path::new(&expected)
        );
    }

    /// Replace `{HOME}` with the home directory. Used to generate expected
    /// strings with the correct home directory in a portable way
    fn replace_home(path: &str) -> String {
        // We're assuming this dependency is correct. This provides portability,
        // so the tests pass on windows
        let home = dirs::home_dir().unwrap();
        let home = home.to_str().unwrap();
        // Sanity check that it gave us a real dir
        assert!(!home.is_empty(), "Home dir is empty");
        path.replace("{HOME}", home)
    }
}
