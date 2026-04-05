use mime::Mime;
use tracing::info;

/// Get a file extension for a MIME type
///
/// If the MIME type is `None` or unknown, return `"data"`
/// TODO this should respect MIME overrides from the config
pub fn mime_to_extension(mime: Option<&Mime>) -> &'static str {
    use mime::{APPLICATION, JSON, TEXT, XML};
    const DEFAULT: &str = "data"; // Everything is data, right?

    // This is duplicated from the TUI matching logic because I didn't want to
    // tie this to syntax highlighting or other language support in the TUI
    let Some(mime) = mime else { return DEFAULT };
    match (mime.type_(), mime.subtype()) {
        // TODO match shit like `application/foo+json`
        (APPLICATION, JSON) => "json",
        (TEXT, XML) => "xml",
        (TEXT, _) => "txt",
        _ => DEFAULT,
    }
}

/// Listen for any exit signals, and return `Ok(())` when any signal is
/// received. This can only fail during initialization.
///
/// TODO dedupe with TUI
#[cfg(unix)]
pub async fn signals() -> anyhow::Result<()> {
    use futures::{FutureExt, future};
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
        use anyhow::Context as _;

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

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    /// Test MIME -> file extension mapping
    #[rstest]
    #[case::json("application/json", "json")]
    // TODO add more cases
    #[case::unknown("unknown/unknown", "data")]
    fn test_mime_to_extension(#[case] mime: Mime, #[case] expected: &str) {
        assert_eq!(mime_to_extension(Some(&mime)), expected);
    }
}
