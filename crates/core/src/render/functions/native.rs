//! Implmentations for native-only functions (template functions that can't
//! run in the web)

use crate::render::{FunctionError, TemplateContext};
use bytes::Bytes;
use futures::{FutureExt, Stream, StreamExt, TryFutureExt, TryStreamExt};
use slumber_template::{RenderError, StreamSource, ValueStream};
use slumber_util::paths::expand_home;
use std::{io, path::PathBuf, process::Stdio};
use tokio::{
    fs::File,
    io::{AsyncRead, AsyncWriteExt},
    process::Command,
};
use tokio_util::io::ReaderStream;
use tracing::{Instrument, debug, debug_span};

/// Run a command in a subprocess
pub fn command(
    context: &TemplateContext,
    command: Vec<String>,
    cwd: Option<String>,
    stdin: Option<Bytes>,
) -> Result<slumber_template::ValueStream, FunctionError> {
    /// Wrap an IO error
    fn io_error(
        program: &str,
        arguments: &[String],
        error: io::Error,
    ) -> RenderError {
        RenderError::from(FunctionError::CommandInit {
            program: program.to_owned(),
            arguments: arguments.to_owned(),
            error,
        })
    }

    let cwd = context.root_dir.join(cwd.unwrap_or_default());
    let [program, arguments @ ..] = command.as_slice() else {
        return Err(FunctionError::CommandEmpty);
    };
    let program = program.clone();
    let arguments = arguments.to_owned();

    // We're going to defer command spawning *and* streaming. Streamed commands
    // shouldn't be spawned until the stream is actually resolved, to prevent
    // running large/slow commands in a preview.
    //
    // We construct a 3-stage stream:
    // - Spawn command
    // - Stream from stdout
    // - Check command status
    let span = debug_span!("command()", ?program, ?arguments);
    let span_ = span.clone(); // Clone so we can attach to the inner stream too
    let future = async move {
        // Spawn the command process

        debug!("Spawning");
        let mut child = Command::new(&program)
            .args(&arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .current_dir(cwd)
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| io_error(&program, &arguments, error))?;

        // Write the stdin to the process
        if let Some(stdin) = stdin {
            child
                .stdin
                .as_mut()
                .expect("Process missing stdin")
                .write_all(&stdin)
                .await
                .map_err(|error| io_error(&program, &arguments, error))?;
        }

        // We have to poll the process (via wait()) and stream from stdout
        // simultaneously. If we just stream from stdout, we never get any
        // output. If we try to wait() then stream from stdout, the stdout
        // buffer may fill up and the process will hang until it's drained. In
        // practice this means we'll poll in a background task, then stream
        // stdout until it's done.
        let stdout = child.stdout.take().expect("stdout not set for child");
        let handle = tokio::spawn(async move { child.wait().await });

        // After stdout is done, we'll check the status code of the process to
        // make sure it succeeded. This gets chained on to the end of
        // the stream
        let status_future = async move {
            let status_result = handle.await;
            debug!(?status_result, "Finished");
            let status = status_result
                .map_err(RenderError::other)? // Join error - task panicked
                // Command error
                .map_err(|error| io_error(&program, &arguments, error))?;
            if status.success() {
                // Since we're chaining onto the end of the output stream, we
                // need to emit empty bytes
                Ok(Bytes::new())
            } else {
                Err(FunctionError::CommandStatus {
                    program,
                    arguments,
                    status,
                }
                .into())
            }
        }
        .instrument(span_);
        Ok(reader_stream(stdout).chain(status_future.into_stream()))
    }
    .instrument(span);

    let stream = future.try_flatten_stream().boxed();

    Ok(ValueStream::Stream {
        source: StreamSource::Command { command },
        stream,
    })
}

/// Load the contents of a file
pub fn file(
    context: &TemplateContext,
    path: String,
) -> slumber_template::ValueStream {
    let path = context.root_dir.join(expand_home(PathBuf::from(path)));
    let source = StreamSource::File { path: path.clone() };
    // Return the file as a stream. If streaming isn't available here, it
    // will be resolved immediately instead. If the file doesn't
    // exist or any other error occurs, the error will be deferred
    // until the data is actually streamed.
    let future = async move {
        let file = File::open(&path)
            .await
            .map_err(|error| FunctionError::File { path, error })?;
        Ok(reader_stream(file))
    };
    ValueStream::Stream {
        source,
        stream: future.try_flatten_stream().boxed(),
    }
}

/// Create a stream from an `AsyncRead` value
fn reader_stream(
    reader: impl AsyncRead,
) -> impl Stream<Item = Result<Bytes, RenderError>> {
    ReaderStream::new(reader).map_err(RenderError::other)
}
