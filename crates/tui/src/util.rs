use crate::{
    message::{Message, MessageSender},
    view::Question,
};
use anyhow::{Context, bail};
use bytes::Bytes;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture},
    terminal::{EnterAlternateScreen, LeaveAlternateScreen},
};
use futures::{FutureExt, future};
use slumber_util::{ResultTraced, ResultTracedAnyhow, paths::expand_home};
use std::{
    env,
    fs::{self, File},
    future::Future,
    io::{self, Write},
    ops::Deref,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, SystemTime},
};
use tokio::{
    fs::OpenOptions,
    io::AsyncWriteExt,
    select,
    sync::oneshot,
    time::{self, MissedTickBehavior},
};
use tokio_util::sync::CancellationToken;
use tracing::{debug, debug_span, error, info, info_span, warn};
use uuid::Uuid;

/// Extension trait for [Result]
pub trait ResultReported<T, E>: Sized {
    /// If this result is an error, send it over the message channel to be
    /// shown the user, and return `None`. If it's `Ok`, return `Some`.
    fn reported(self, messages_tx: &MessageSender) -> Option<T>;
}

impl<T, E> ResultReported<T, E> for Result<T, E>
where
    E: Into<anyhow::Error>,
{
    fn reported(self, messages_tx: &MessageSender) -> Option<T> {
        match self {
            Ok(value) => Some(value),
            Err(err) => {
                // Trace this too, because anything that should be shown to the
                // user should also be logged
                let err = err.into();
                error!(error = err.deref());
                messages_tx.send(Message::Error { error: err });
                None
            }
        }
    }
}

/// A flag that starts as false and can only be enabled
#[derive(Copy, Clone, Debug, Default, derive_more::Deref)]
pub struct Flag(bool);

impl Flag {
    /// Enable the flag
    pub fn set(&mut self) {
        self.0 = true;
    }

    /// Disable the flag
    pub fn unset(&mut self) {
        self.0 = false;
    }
}

/// A temporary file. The file is created with a random name when the struct is
/// initialized, and deleted when the struct is dropped.
#[derive(Debug)]
pub struct TempFile {
    path: PathBuf,
}

impl TempFile {
    /// Create a new temporary file with the given contents
    pub fn new(contents: &[u8]) -> anyhow::Result<Self> {
        Self::with_file(|file| file.write_all(contents))
    }

    /// Create a new temporary file and call a function to initialize it with
    /// data. This is used for writing a ratatui `Text` object to a file, which
    /// isn't accessible as a single chunk of bytes.
    pub fn with_file(
        mut writer: impl FnMut(&mut File) -> io::Result<()>,
    ) -> anyhow::Result<Self> {
        let path = env::temp_dir().join(format!("slumber-{}", Uuid::new_v4()));
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path)
            .with_context(|| {
                format!("Error creating temporary file `{}`", path.display())
            })?;
        writer(&mut file).with_context(|| {
            format!("Error writing to temporary file `{}`", path.display())
        })?;
        Ok(Self { path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path)
            .with_context(|| {
                format!(
                    "Error deleting temporary file `{}`",
                    self.path.display()
                )
            })
            .traced();
    }
}

/// Run a **blocking** subprocess that will take over the terminal. Used
/// for opening an external editor or pager. Useful for terminal editors since
/// they'll take over the whole screen. Potentially annoying for GUI editors
/// that block, but we'll just hope the command is fire-and-forget. If this
/// becomes an issue we can try to detect if the subprocess took over the
/// terminal and cut it loose if not, or add a config field for it.
pub fn yield_terminal(
    mut command: Command,
    messages_tx: &MessageSender,
) -> anyhow::Result<()> {
    let span = info_span!("Running command", ?command).entered();
    let error_context = format!("Error spawning command `{command:?}`");

    // Clear the terminal so the buffer is empty. This forces a total redraw
    // when we take it back over. Otherwise ratatui would think the screen is
    // still intact and not draw anything
    messages_tx.send(Message::ClearTerminal);

    // Reset terminal to normal
    restore_terminal()?;

    // Run the command. Make sure to perform cleanup even if the command
    // failed
    let command_result = command
        .status()
        .map_err(anyhow::Error::from)
        .and_then(|status| {
            if status.success() {
                info!(status = status.code(), "Command succeeded");
                Ok(())
            } else {
                // It would be nice to log stdout/stderr here, but we can't
                // capture them because some commands (e.g. `less`) will behave
                // differently when redirected
                error!(status = status.code(), "Command failed");
                // Show the error to the user
                Err(anyhow::anyhow!("Command failed with status {status}"))
            }
        })
        .context(error_context);

    // Some editors *cough* vim *cough* dump garbage to the event buffer on
    // exit. I've never figured out what actually causes it, but a simple
    // solution is to dump all events in the buffer before returning to
    // Slumber. It's possible we lose some real user input here (e.g. if
    // other events were queued behind the event to open the editor).
    clear_event_buffer();
    initialize_terminal()?; // Take it back over
    drop(span);

    command_result
}

/// Set up terminal for TUI
pub fn initialize_terminal() -> anyhow::Result<()> {
    debug!("Initializing terminal");
    crossterm::terminal::enable_raw_mode()?;
    crossterm::execute!(
        io::stdout(),
        EnterAlternateScreen,
        EnableMouseCapture
    )?;
    Ok(())
}

/// Return terminal to initial state
pub fn restore_terminal() -> anyhow::Result<()> {
    debug!("Restoring terminal");
    crossterm::terminal::disable_raw_mode()?;
    crossterm::execute!(
        io::stdout(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    Ok(())
}

/// Clear all input events in the terminal event buffer
pub fn clear_event_buffer() {
    while let Ok(true) = event::poll(Duration::from_millis(0)) {
        let _ = event::read();
    }
}

/// Listen for any exit signals, and return `Ok(())` when any signal is
/// received. This can only fail during initialization.
#[cfg(unix)]
pub async fn signals() -> anyhow::Result<()> {
    use itertools::Itertools;
    use tokio::signal::unix::{Signal, SignalKind, signal};

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

/// Watch a file and call a callback when it changes
///
/// This is a simple async polling file watcher. The `notify` crate is neat and
/// all, but it's overkill for what we need. Home-rolled polling is better
/// because:
/// - We're only watching one file at a time, so it's cheap
/// - It's cross-platform
/// - It works with swap-based editors (e.g. vim) that replace files on save
/// - It's async, whereas `notify` uses a separate thread
pub async fn watch_file(path: PathBuf, f: impl Fn()) {
    /// Time between polls on the collection file. This is a tradeoff between
    /// CPU/IO usage and responsiveness. Shorter interval also speeds up tests
    /// that rely on reloading.
    const FILE_POLL_INTERVAL: Duration = Duration::from_millis(100);

    // Watch the modified timestamp for changes
    info!(?path, "Watching file for changes");

    let mut has_logged_error = false;
    let mut get_last_modified = || -> SystemTime {
        // This call should be very fast, so not worth asyncing. Async fs
        // operations spawn background threads in tokio.
        let mut result =
            std::fs::metadata(&path).and_then(|metadata| metadata.modified());
        // If this is the first time seeing the error, log it.
        // We don't want to log the same error every 100ms
        if !has_logged_error {
            result = result.traced();
            has_logged_error = true;
        }
        // Use a placeholder time that will always be old if
        // we get a valid time later on
        result.unwrap_or(SystemTime::UNIX_EPOCH)
    };

    let mut last_modified = get_last_modified();
    let mut interval = time::interval(FILE_POLL_INTERVAL);
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        let lm = get_last_modified();
        if lm != last_modified {
            info!(?path, "File changed, reloading");
            last_modified = lm;
            f();
        }
        interval.tick().await;
    }
}

/// Make a future cancellable with the given token
pub fn cancellable(
    cancel_token: &CancellationToken,
    future: impl 'static + Future<Output = ()>,
) -> impl 'static + Future<Output = ()> {
    let cancel_token = cancel_token.clone();
    async move {
        select! {
            () = future => {},
            () = cancel_token.cancelled() => {}
        }
    }
}

/// Save some data to disk. This will:
/// - Ask the user for a path
/// - Attempt to save a *new* file
/// - If the file already exists, ask for confirmation
/// - If confirmed, overwrite existing
pub async fn save_file(
    messages_tx: MessageSender,
    default_path: Option<String>,
    data: Bytes,
) -> anyhow::Result<()> {
    // If the user closed the prompt, just exit
    let Some(path) =
        text_question(&messages_tx, "Enter a path for the file", default_path)
            .await
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

/// Run a command, optionally piping some stdin to it. This will use given shell
/// (e.g. `["sh", "-c"]`) to execute the command, or parse+run it natively if no
/// shell is set. The shell should generally come from the config, but is
/// taken as param for testing.
pub async fn run_command(
    shell: &[String],
    command_str: &str,
    stdin: Option<&[u8]>,
) -> anyhow::Result<Vec<u8>> {
    let _ = debug_span!("Command", command = command_str).entered();

    let mut command = if let [program, args @ ..] = shell {
        // Invoke the shell with our command as the final arg
        let mut command = tokio::process::Command::new(program);
        command.args(args).arg(command_str);
        command
    } else {
        // Shell command is empty - we should execute the command directly.
        // We'll have to do our own parsing of it
        let tokens = shell_words::split(command_str)?;
        let [program, args @ ..] = tokens.as_slice() else {
            bail!("Command is empty")
        };
        let mut command = tokio::process::Command::new(program);
        command.args(args);
        command
    };

    let mut process = command
        // Stop the command on drop. This will leave behind a zombie process,
        // but tokio should reap it in the background. See method docs
        .kill_on_drop(true)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    if let Some(stdin) = stdin {
        // If writing to stdin fails, it's probably because the process exited
        // immediately. This typically indicates some other error. We _don't_
        // want to show the stdin error, because it will mask the actual error.
        let _ = process
            .stdin
            .as_mut()
            .expect("Process missing stdin")
            .write_all(stdin)
            .await;
    }
    let output = process.wait_with_output().await?;
    debug!(
        status = ?output.status,
        stdout = %String::from_utf8_lossy(&output.stdout),
        stderr = %String::from_utf8_lossy(&output.stderr),
        "Command complete"
    );
    if !output.status.success() {
        let stderr = std::str::from_utf8(&output.stderr).unwrap_or_default();
        bail!("{stderr}");
    }
    Ok(output.stdout)
}

/// Ask the user for some text input and wait for a response. Return `None` if
/// the prompt is closed with no input.
async fn text_question(
    messages_tx: &MessageSender,
    message: impl ToString,
    default: Option<String>,
) -> Option<String> {
    let (tx, rx) = oneshot::channel();
    messages_tx.send(Message::Question(Question::Text {
        message: message.to_string(),
        default,
        channel: tx.into(),
    }));
    // Error indicates no response, we can throw that away
    rx.await.ok()
}

/// Ask the user a yes/no question and wait for a response
pub async fn confirm(
    messages_tx: &MessageSender,
    message: impl ToString,
) -> bool {
    let (tx, rx) = oneshot::channel();
    messages_tx.send(Message::Question(Question::Confirm {
        message: message.to_string(),
        channel: tx.into(),
    }));
    // Error means we got ghosted :( RUDE!
    rx.await.unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::MessageQueue;
    use rstest::rstest;
    use slumber_config::CommandsConfig;
    use slumber_util::{TempDir, assert_matches, temp_dir};
    use tokio::fs;

    /// Test various cases of save_file
    #[rstest]
    #[case::new_file(false, false)]
    #[case::old_file_remain(true, false)]
    #[case::old_file_overwrite(true, true)]
    #[tokio::test]
    async fn test_save_file(
        temp_dir: TempDir,
        #[case] exists: bool,
        #[case] overwrite: bool,
    ) {
        let expected_path = temp_dir.join("test.txt");
        if exists {
            fs::write(&expected_path, b"already here").await.unwrap();
        }

        // We need to run two futures concurrently:
        // - save_file() procedure
        // - Respondent that will pop the prompt messages and handle them
        let mut messages = MessageQueue::new();
        let save_file_fut = save_file(
            messages.tx(),
            Some("default.txt".into()),
            b"hello!".as_slice().into(),
        );

        let assertions_fut = async {
            // First we expect a prompt for the file path
            let (message, default, channel) = assert_matches!(
                messages.pop_wait().await,
                Some(Message::Question(Question::Text {
                    message, default, channel, ..
                })) => {
                    (message, default, channel)
                },
            );
            assert_eq!(&message, "Enter a path for the file");
            assert_eq!(default.as_deref(), Some("default.txt"));
            channel.reply(expected_path.to_str().unwrap().to_owned());

            if exists {
                // Now we expect a confirmation prompt
                let (message, channel) = assert_matches!(
                    messages.pop_wait().await,
                    Some(Message::Question(Question::Confirm { message, channel })) => {
                        (message, channel)
                    },
                );
                assert_eq!(
                    message,
                    format!(
                        "`{}` already exists, overwrite?",
                        expected_path.display()
                    )
                );
                channel.reply(overwrite);
            }
        };

        // Run the two futures together
        let (result, ()) = future::join(save_file_fut, assertions_fut).await;
        result.unwrap();

        // Now the file should be created
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

    #[rstest]
    #[case::default_shell(
        &CommandsConfig::default().shell,
        "echo test | head -c 1",
        "t",
    )]
    #[case::no_shell(&[], "echo -n test | head -c 1", "test | head -c 1")]
    // I don't feel like getting this case to work with powershell
    #[cfg_attr(not(windows), case::custom_shell(
        &["bash".into(), "-c".into()],
        "echo test | head -c 1",
        "t",
    ))]
    #[tokio::test]
    async fn test_run_command(
        #[case] shell: &[String],
        #[case] command: &str,
        #[case] expected: &str,
    ) {
        let bytes = run_command(shell, command, None).await.unwrap();
        let s = std::str::from_utf8(&bytes).unwrap();
        assert_eq!(s, expected);
    }
}
