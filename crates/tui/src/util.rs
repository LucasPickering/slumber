use crate::{
    context::TuiContext,
    message::{Message, MessageSender},
    view::{Confirm, ViewContext},
};
use anyhow::{Context, bail};
use bytes::Bytes;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture},
    terminal::{EnterAlternateScreen, LeaveAlternateScreen},
};
use editor_command::EditorBuilder;
use futures::{FutureExt, future};
use mime::Mime;
use slumber_core::{template::Prompt, util::doc_link};
use slumber_util::{ResultTraced, paths::expand_home};
use std::{
    env,
    fs::{self, File},
    future::Future,
    io::{self, Write},
    ops::Deref,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::LazyLock,
    time::Duration,
};
use tokio::{
    fs::OpenOptions,
    io::AsyncWriteExt,
    select,
    sync::oneshot,
    task::{self, JoinHandle},
};
use tokio_util::sync::CancellationToken;
use tracing::{debug, debug_span, error, info, info_span, warn};
use uuid::Uuid;

/// Token to manage cancellation of background tasks
pub static CANCEL_TOKEN: LazyLock<CancellationToken> =
    LazyLock::new(CancellationToken::new);

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
                format!("Error creating temporary file {path:?}")
            })?;
        writer(&mut file).with_context(|| {
            format!("Error writing to temporary file {path:?}")
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
                format!("Error deleting temporary file {:?}", self.path)
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
                Err(anyhow::anyhow!("Command failed with status {}", status))
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

    // Redraw immediately. The main loop will probably be in the tick
    // timeout when we go back to it, so that adds a 250ms delay to
    // redrawing the screen that we want to skip.
    messages_tx.send(Message::Draw);

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

/// Spawn a task on the main thread. Most tasks can use this because the app is
/// generally I/O bound, so we can handle all async stuff on a single thread.
/// The UI will be redrawn when the task is done. This redraw may be redundant,
/// but it's thorough and the cost is minimal.
pub fn spawn(future: impl 'static + Future<Output = ()>) -> JoinHandle<()> {
    task::spawn_local(async move {
        select! {
            () = future => {
                // Assume the task updated _something_ visible to the user,
                // so trigger a redraw here
                ViewContext::messages_tx().send(Message::Draw);
            },
            () = CANCEL_TOKEN.cancelled() => {},
        }
    })
}

/// Spawn a fallible task. If it fails, report the error to the user
pub fn spawn_result(
    future: impl 'static + Future<Output = anyhow::Result<()>>,
) -> JoinHandle<()> {
    spawn(async move {
        future.await.reported(&ViewContext::messages_tx());
    })
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

/// Get a command to open the given file in the user's configured editor.
/// Default editor is `vim`. Return an error if the command couldn't be built.
pub fn get_editor_command(file: &Path) -> anyhow::Result<Command> {
    EditorBuilder::new()
        // Config field takes priority over environment variables
        .source(TuiContext::get().config.editor.as_deref())
        .environment()
        .source(Some("vim"))
        .path(file)
        .build()
        .with_context(|| {
            format!(
                "Error opening editor; see {}",
                doc_link("user_guide/tui/editor"),
            )
        })
}

/// Get a command to open the given file in the user's configured file pager.
/// Default is `less` on Unix, `more` on Windows. Return an error if the command
/// couldn't be built.
pub fn get_pager_command(
    file: &Path,
    mime: Option<&Mime>,
) -> anyhow::Result<Command> {
    // Use a built-in pager
    let default = if cfg!(windows) { "more" } else { "less" };

    // Select command from the config based on content type
    let config_command =
        mime.and_then(|mime| TuiContext::get().config.pager.get(mime));

    EditorBuilder::new()
        // Config field takes priority over environment variables
        .source(config_command)
        .source(env::var("PAGER").ok())
        .source(Some(default))
        .path(file)
        .build()
        .with_context(|| {
            format!(
                "Error opening pager; see {}",
                doc_link("user_guide/tui/editor"),
            )
        })
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
        bail!("{}", stderr);
    }
    Ok(output.stdout)
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
pub async fn confirm(
    messages_tx: &MessageSender,
    message: impl ToString,
) -> bool {
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
    use crate::test_util::{TestHarness, harness};
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
            b"hello!".as_slice().into(),
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
