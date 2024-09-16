use anyhow::{anyhow, Context};
use std::{
    borrow::Cow,
    fs,
    path::{Path, PathBuf},
};

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
        get_repo_root().join("data/")
    }
    #[cfg(not(debug_assertions))]
    {
        path
    }
}

/// Ensure the parent directory of a file path exists
pub fn create_parent(path: &Path) -> anyhow::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        anyhow!("Cannot create directory for path {path:?}; it has no parent")
    })?;
    fs::create_dir_all(parent)
        .context("Error creating directory {parent:?}")?;
    Ok(())
}

/// Get path to the root of the git repo. This is needed because this crate
/// doesn't live at the repo root, so we can't use `CARGO_MANIFEST_DIR`. Path
/// will be cached so subsequent calls are fast. If the path can't be found,
/// fall back to the current working directory instead. Always returns an
/// absolute path.
#[cfg(any(debug_assertions, test))]
pub(crate) fn get_repo_root() -> &'static Path {
    use crate::util::ResultTraced;
    use std::{process::Command, sync::OnceLock};

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
