use crate::{
    message::{Message, MessageSender},
    view::Confirm,
};
use anyhow::{anyhow, Context};
use derive_more::DerefMut;
use futures::{future, FutureExt};
use slumber_core::{
    http::RequestError,
    template::{ChainError, Prompt},
    util::{expand_home, ResultTraced},
};
use std::{
    env, io,
    ops::Deref,
    path::{Path, PathBuf},
    process::Command,
};
use tokio::{fs::OpenOptions, io::AsyncWriteExt, sync::oneshot};
use tracing::{debug, info, warn};

/// Extension trait for [Result]
pub trait ResultReported<T, E>: Sized {
    /// If this result is an error, send it over the message channel to be
    /// shown the user, and return `None`. If it's `Ok`, return `Some`.
    fn reported(self, messages_tx: &MessageSender) -> Option<T>;
}

// This is deliberately *not* implemented for non-anyhow errors, because we only
// want to trace errors that have full context attached
impl<T> ResultReported<T, anyhow::Error> for anyhow::Result<T> {
    fn reported(self, messages_tx: &MessageSender) -> Option<T> {
        match self {
            Ok(value) => Some(value),
            Err(error) => {
                messages_tx.send(Message::Error { error });
                None
            }
        }
    }
}

impl<T> ResultReported<T, RequestError> for Result<T, RequestError> {
    fn reported(self, messages_tx: &MessageSender) -> Option<T> {
        self.map_err(anyhow::Error::from).reported(messages_tx)
    }
}

impl<T> ResultReported<T, ChainError> for Result<T, ChainError> {
    fn reported(self, messages_tx: &MessageSender) -> Option<T> {
        self.map_err(anyhow::Error::from).reported(messages_tx)
    }
}

/// A value that can be replaced in-place. This is useful for two purposes:
/// - Transferring ownership of values from old to new
/// - Dropping the old value before creating the new one
///
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
    pub fn replace(&mut self, f: impl FnOnce(T) -> T) {
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

/// Listen for any exit signals, and return `Ok(())` when any signal is
/// received. This can only fail during initialization.
#[cfg(unix)]
pub async fn signals() -> anyhow::Result<()> {
    use itertools::Itertools;
    use tokio::signal::unix::{signal, Signal, SignalKind};

    let signals: Vec<(Signal, SignalKind)> = [
        SignalKind::interrupt(),
        SignalKind::hangup(),
        SignalKind::terminate(),
        SignalKind::quit(),
    ]
    .into_iter()
    .map::<anyhow::Result<_>, _>(|kind| {
        let signal = signal(kind).with_context(|| {
            format!("Error initializing listener for signal `{kind:?}`")
        })?;
        Ok((signal, kind))
    })
    .try_collect()?;
    let futures = signals
        .into_iter()
        .map(|(mut signal, kind)| async move {
            signal.recv().await;
            info!(?kind, "Received signal");
        })
        .map(FutureExt::boxed);
    future::select_all(futures).await;
    Ok(())
}

/// Listen for any exit signals, and return `Ok(())` when any signal is
/// received. This can only fail during initialization.
#[cfg(windows)]
pub async fn signals() -> anyhow::Result<()> {
    use tokio::signal::windows::{ctrl_break, ctrl_c, ctrl_close};

    let (mut s1, mut s2, mut s3) = (ctrl_c()?, ctrl_break()?, ctrl_close()?);
    let futures = vec![s1.recv().boxed(), s2.recv().boxed(), s3.recv().boxed()];
    future::select_all(futures).await;
    info!("Received exit signal");
    Ok(())
}

/// Save some data to disk. This will:
/// - Ask the user for a path
/// - Attempt to save a *new* file
/// - If the file already exists, ask for confirmation
/// - If confirmed, overwrite existing
pub async fn save_file(
    messages_tx: MessageSender,
    default_path: Option<String>,
    data: Vec<u8>,
) -> anyhow::Result<()> {
    // If the user closed the prompt, just exit
    let Some(path) =
        prompt(&messages_tx, "Enter a path for the file", default_path).await
    else {
        return Ok(());
    };

    // If the user input nothing, assume they just want to exit
    if path.is_empty() {
        return Ok(());
    }

    let path = expand_home(PathBuf::from(path)); // Expand ~
    let result = {
        // Attempt to open the file *if it doesn't exist already*
        let result = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .await;

        match result {
            Ok(file) => Ok(file),
            // If the file already exists, ask for confirmation to overwrite
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                warn!(?path, "File already exists, asking to overwrite");

                // Hi, sorry, follow up question. Are you sure?
                if confirm(
                    &messages_tx,
                    format!("`{}` already exists, overwrite?", path.display()),
                )
                .await
                {
                    // REALLY attempt to open the file
                    OpenOptions::new()
                        .create(true)
                        .write(true)
                        .truncate(true)
                        .open(&path)
                        .await
                } else {
                    return Ok(());
                }
            }
            Err(error) => Err(error),
        }
    };

    debug!(?path, bytes = data.len(), "Writing to file");
    async {
        let mut file = result?;
        file.write_all(&data).await?;
        file.flush().await
    }
    .await
    .with_context(|| format!("Error writing to file `{}`", path.display()))
    .traced()?;

    // It might be nice to show the full path here, but it's not trivial to get
    // that. The stdlib has fs::canonicalize, but it does more than we need
    // (specifically it resolves symlinks), which might be confusing
    messages_tx.send(Message::Notify(format!("Saved to {}", path.display())));
    Ok(())
}

/// Get a command to open the given file in the user's configured editor. Return
/// an error if the user has no editor configured
pub fn get_editor_command(file: &Path) -> anyhow::Result<Command> {
    let command = env::var("VISUAL").or(env::var("EDITOR")).map_err(|_| {
        anyhow!(
            "No editor configured. Please set the `VISUAL` or `EDITOR` \
            environment variable"
        )
    })?;
    let mut splits = command.split(' ');
    let editor = splits.next().expect("`split` returns at least one value");
    let mut command = Command::new(editor);
    command.args(splits).arg(file);
    Ok(command)
}

/// Ask the user for some text input and wait for a response. Return `None` if
/// the prompt is closed with no input.
async fn prompt(
    messages_tx: &MessageSender,
    message: impl ToString,
    default: Option<String>,
) -> Option<String> {
    let (tx, rx) = oneshot::channel();
    messages_tx.send(Message::PromptStart(Prompt {
        message: message.to_string(),
        default,
        sensitive: false,
        channel: tx.into(),
    }));
    // Error indicates no response, we can throw that away
    rx.await.ok()
}

/// Ask the user a yes/no question and wait for a response
async fn confirm(messages_tx: &MessageSender, message: impl ToString) -> bool {
    let (tx, rx) = oneshot::channel();
    let confirm = Confirm {
        message: message.to_string(),
        channel: tx.into(),
    };
    messages_tx.send(Message::ConfirmStart(confirm));
    // Error means we got ghosted :( RUDE!
    rx.await.unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{harness, TestHarness};
    use itertools::Itertools;
    use rstest::rstest;
    use slumber_core::{
        assert_err, assert_matches,
        test_util::{temp_dir, EnvGuard, TempDir},
    };
    use std::ffi::OsStr;
    use tokio::fs;

    /// Test reading editor command from VISUAL/EDITOR env vars
    #[rstest]
    #[case::visual(Some("ted"), Some("fred"), "ted", &[])]
    #[case::editor(None, Some("fred"), "fred", &[])]
    #[case::with_args(None, Some("ned --wait 60s"), "ned", &["--wait", "60s"])]
    // This case is actually a bug, but I don't think it's worth the effort of
    // engineering around. I added this test case for completeness
    #[case::with_args_quoted(
        None, Some("ned '--wait 60s'"), "ned", &["'--wait", "60s'"],
    )]
    fn test_get_editor(
        #[case] env_visual: Option<&str>,
        #[case] env_editor: Option<&str>,
        #[case] expected_program: &str,
        #[case] expected_args: &[&str],
    ) {
        let file_name = "file.yml";
        // Make sure we're not competing with the other tests that want to set
        // these env vars
        let command = {
            let _guard = EnvGuard::lock([
                ("VISUAL", env_visual),
                ("EDITOR", env_editor),
            ]);
            get_editor_command(Path::new(file_name))
        }
        .unwrap();
        let mut expected_args = expected_args.to_owned();
        expected_args.push(file_name);
        assert_eq!(command.get_program(), expected_program);
        assert_eq!(
            command
                .get_args()
                .filter_map(OsStr::to_str)
                .collect_vec()
                .as_slice(),
            expected_args
        );
    }

    /// Test when VISUAL/EDITOR env vars are empty
    #[test]
    fn test_get_editor_error() {
        // Make sure we're not competing with the other tests that want to set
        // these env vars
        let result = {
            let _guard =
                EnvGuard::lock([("VISUAL", None::<String>), ("EDITOR", None)]);
            get_editor_command(Path::new("file.yml"))
        };
        assert_err!(result, "No editor configured");
    }

    /// Test various cases of save_file
    #[rstest]
    #[case::new_file(false, false)]
    #[case::old_file_remain(true, false)]
    #[case::old_file_overwrite(true, true)]
    #[tokio::test]
    async fn test_save_file(
        mut harness: TestHarness,
        temp_dir: TempDir,
        #[case] exists: bool,
        #[case] overwrite: bool,
    ) {
        let expected_path = temp_dir.join("test.txt");
        if exists {
            fs::write(&expected_path, b"already here").await.unwrap();
        }

        // This will run in the background and save the file after prompts
        let handle = tokio::spawn(save_file(
            harness.messages_tx().clone(),
            Some("default.txt".into()),
            b"hello!".to_vec(),
        ));

        // First we expect a prompt for the file path
        let prompt = assert_matches!(
            harness.pop_message_wait().await,
            Message::PromptStart(prompt) => prompt,
        );
        assert_eq!(&prompt.message, "Enter a path for the file");
        assert_eq!(prompt.default.as_deref(), Some("default.txt"));
        prompt
            .channel
            .respond(expected_path.to_str().unwrap().to_owned());

        if exists {
            // Now we expect a confirmation prompt
            let confirm = assert_matches!(
                harness.pop_message_wait().await,
                Message::ConfirmStart(confirm) => confirm,
            );
            assert_eq!(
                confirm.message,
                format!(
                    "`{}` already exists, overwrite?",
                    expected_path.display()
                )
            );
            confirm.channel.respond(overwrite);
        }

        // Now the file should be created
        handle
            .await
            .expect("Task dropped")
            .expect("save_file failed");
        let expected = if !exists || overwrite {
            "hello!"
        } else {
            "already here"
        };
        assert_eq!(
            &fs::read_to_string(&expected_path).await.unwrap(),
            expected,
            "{expected_path:?}"
        );
    }
}
