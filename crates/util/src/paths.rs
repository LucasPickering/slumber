use path_clean::PathClean;
use std::{
    borrow::Cow,
    fs, io,
    path::{Path, PathBuf},
};

/// Set this environment variable to change the data directory. Useful for tests
#[cfg(debug_assertions)]
pub const DATA_DIRECTORY_ENV_VARIABLE: &str = "SLUMBER_DB";

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

/// Get the path of the directory to contain log files. **Directory
/// may not exist yet**, caller must create it.
pub fn log_directory() -> PathBuf {
    // State dir is only present on windows, but cache dir will be present on
    // all platforms
    // https://docs.rs/dirs/latest/dirs/fn.state_dir.html
    // https://docs.rs/dirs/latest/dirs/fn.cache_dir.html
    debug_or(
        dirs::state_dir()
            .unwrap_or_else(|| dirs::cache_dir().unwrap())
            .join("slumber"),
    )
}

/// Get the path to the primary log file. **Parent direct may not exist yet,**
/// caller must create it.
pub fn log_file() -> PathBuf {
    log_directory().join("slumber.log")
}

/// Get the path to the backup log file **Parent direct may not exist yet,**
/// caller must create it.
pub fn log_file_old() -> PathBuf {
    log_directory().join("slumber.log.old")
}

/// In debug mode, use a local directory for all files. In release, use the
/// given path.
fn debug_or(path: PathBuf) -> PathBuf {
    #[cfg(debug_assertions)]
    {
        let _ = path; // Remove unused warning
        // Check the env var, for tests
        std::env::var(DATA_DIRECTORY_ENV_VARIABLE)
            .map(PathBuf::from)
            .unwrap_or_else(|_| get_repo_root().join("data/"))
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
