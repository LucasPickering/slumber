//! Helper structs and functions for building components

pub mod highlight;
pub mod preview;

use crate::{
    message::{HttpMessage, Message, MessageSender},
    util::{ResultReported, TempFile, syntax::SyntaxType},
    view::ViewContext,
};
use async_trait::async_trait;
use chrono::{
    DateTime, Duration, Local, Utc,
    format::{DelayedFormat, StrftimeItems},
};
use derive_more::From;
use itertools::Itertools;
use mime::Mime;
use ratatui::text::{Line, Text};
use slumber_core::{
    collection::{CollectionError, CollectionFile, RecipeId},
    http::RequestId,
    render::{Prompter, SelectOption},
};
use slumber_template::Value;
use std::{io::Write, sync::Arc};
use tokio::sync::oneshot;
use tracing::{error, trace};

/// Container for the state the view needs to show a collection load error
#[derive(Debug)]
pub struct InvalidCollection {
    pub file: CollectionFile,
    pub error: Arc<CollectionError>,
}

/// A question posed to the user. [Prompt] is used exclusively for request
/// building, while this value is used for any other kind of input requested
/// from the user.
#[derive(Debug)]
pub enum Question {
    /// Yes/no question
    Confirm {
        message: String,
        channel: ReplyChannel<bool>,
    },
    /// Question with text response
    Text {
        message: String,
        default: Option<String>,
        channel: ReplyChannel<String>,
    },
}

/// Use the message stream to prompt the user for input when needed for a
/// template. The message will be routed to the view so it can show the prompt,
/// and the given returner will be used to send the submitted value back.
#[derive(Debug)]
pub struct TuiPrompter {
    /// Recipe of the request being built. This has the same flaws as
    /// `request_id` related to triggered requests.
    recipe_id: RecipeId,
    /// Request being built with this prompter. Each request gets its own
    /// TemplateContext, which gets a new prompter. This allows us to group the
    /// prompts by request in the UI.
    ///
    /// **However**, triggered requests will use the same context as the
    /// triggerer, so all triggered requests will be tagged with their parent.
    /// This is a flaw but may be better UX because it keeps all the prompts
    /// require for the parent in a single form.
    request_id: RequestId,
    messages_tx: MessageSender,
}

impl TuiPrompter {
    pub fn new(
        recipe_id: RecipeId,
        request_id: RequestId,
        messages_tx: MessageSender,
    ) -> Self {
        Self {
            recipe_id,
            request_id,
            messages_tx,
        }
    }
}

#[async_trait(?Send)]
impl Prompter for TuiPrompter {
    async fn prompt_text(
        &self,
        message: String,
        default: Option<String>,
        sensitive: bool,
    ) -> Option<String> {
        let (tx, rx) = oneshot::channel();
        let prompt = Prompt::Text {
            message,
            default,
            sensitive,
            channel: ReplyChannel(tx),
        };
        self.messages_tx.send(HttpMessage::Prompt {
            recipe_id: self.recipe_id.clone(),
            request_id: self.request_id,
            prompt,
        });
        rx.await.ok()
    }

    async fn prompt_select(
        &self,
        message: String,
        options: Vec<SelectOption>,
    ) -> Option<Value> {
        let (tx, rx) = oneshot::channel();
        let prompt = Prompt::Select {
            message,
            options,
            channel: ReplyChannel(tx),
        };
        self.messages_tx.send(HttpMessage::Prompt {
            recipe_id: self.recipe_id.clone(),
            request_id: self.request_id,
            prompt,
        });
        rx.await.ok()
    }
}

/// A prompter that returns a static value; used for template previews, where
/// user interaction isn't possible
#[derive(Debug)]
pub struct PreviewPrompter;

#[async_trait(?Send)]

impl Prompter for PreviewPrompter {
    async fn prompt_text(
        &self,
        _message: String,
        default: Option<String>,
        _sensitive: bool,
    ) -> Option<String> {
        Some(default.unwrap_or_else(|| "<prompt>".into()))
    }

    async fn prompt_select(
        &self,
        _message: String,
        _options: Vec<SelectOption>,
    ) -> Option<Value> {
        Some("<select>".into())
    }
}

/// Data defining a prompt to be presented to the user
#[derive(Debug)]
pub enum Prompt {
    /// Ask the user for text input
    Text {
        /// Tell the user what we're asking for
        message: String,
        /// Value used to pre-populate the text box
        default: Option<String>,
        /// Should the value the user is typing be masked? E.g. password input
        sensitive: bool,
        /// How the prompter will pass the answer back
        channel: ReplyChannel<String>,
    },
    /// Ask the user to pick a value from a list
    Select {
        /// Tell the user what we're asking for
        message: String,
        /// List of choices the user can pick from. This will never be empty.
        options: Vec<SelectOption>,
        /// How the prompter will pass the answer back. The returned value is
        /// the `value` field from the selected [SelectOption]
        channel: ReplyChannel<Value>,
    },
}

/// Channel used to return a reply to a one-time request
#[derive(Debug, From)]
pub struct ReplyChannel<T>(oneshot::Sender<T>);

impl<T> ReplyChannel<T> {
    /// Return the value that the user gave
    pub fn reply(self, reply: T) {
        // This error *shouldn't* ever happen, because the templating task
        // stays open until it gets a reply
        if self.0.send(reply).is_err() {
            error!("Reply listener dropped");
        }
    }
}

/// Convert a `&str` to an **owned** `Text` object. This is functionally the
/// same as `s.to_owned().into()`, but prevents having to clone the entire text
/// twice (once to create the `String` and again when breaking it apart into
/// lines).
pub fn str_to_text(s: &str) -> Text<'static> {
    s.lines()
        .map(|line| Line::from(line.to_owned()))
        .collect_vec()
        .into()
}

/// Open a [Text] object in the user's external pager. This will write the text
/// to a random temporary file, without having to copy the contents. If an
/// error occurs, it will be traced and reported to the user. `content_type`
/// should be the value of an associated `Content-Type` header, if any. This is
/// used to select the correct pager command.
pub fn view_text(text: &Text, mime: Option<Mime>) {
    // Write text to the file line-by-line. This avoids having to copy the bytes
    // to a single chunk of bytes just to write them out
    let Some(file) = TempFile::with_file(
        |file| {
            for line in &text.lines {
                for span in &line.spans {
                    file.write_all(span.content.as_bytes())?;
                }
                // Every line gets a line ending, so we end up with a trailing
                // one
                file.write_all(b"\n")?;
            }
            Ok(())
        },
        mime.as_ref().and_then(|mime| {
            SyntaxType::from_mime(ViewContext::config().mime_overrides(), mime)
        }),
    )
    .reported(&ViewContext::messages_tx()) else {
        return;
    };
    trace!(?file, "Wrote body to temporary file");
    ViewContext::push_message(Message::FileView { file, mime });
}

/// Format a datetime for the user
pub fn format_time(time: &DateTime<Utc>) -> DelayedFormat<StrftimeItems<'_>> {
    time.with_timezone(&Local).format("%b %-d %H:%M:%S")
}

/// Format a duration for the user
pub fn format_duration(duration: &Duration) -> String {
    let ms = duration.num_milliseconds();
    if ms < 1000 {
        format!("{ms}ms")
    } else {
        format!("{:.2}s", ms as f64 / 1000.0)
    }
}

/// Format a byte total, e.g. 1_000_000 -> 1 MB
pub fn format_byte_size(size: usize) -> String {
    const K: usize = 10usize.pow(3);
    const M: usize = 10usize.pow(6);
    const G: usize = 10usize.pow(9);
    const T: usize = 10usize.pow(12);
    let (denom, suffix) = match size {
        ..K => return format!("{size} B"),
        K..M => (K, "K"),
        M..G => (M, "M"),
        G..T => (G, "G"),
        T.. => (T, "T"),
    };
    let size = size as f64 / denom as f64;
    format!("{size:.1} {suffix}B")
}

/// Get a minified name for a type. Common prefixes are stripped from the type
/// to reduce clutter
pub fn format_type_name(type_name: &str) -> String {
    type_name
        .replace("slumber_tui::view::common::", "")
        .replace("slumber_tui::view::component::", "")
        .replace("slumber_tui::view::test_util::", "")
        .replace("slumber_tui::view::util::", "")
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case::zero(0, "0 B")]
    #[case::one(1, "1 B")]
    #[case::almost_kb(999, "999 B")]
    #[case::kb(1000, "1.0 KB")]
    #[case::kb_round_down(1049, "1.0 KB")]
    #[case::kb_round_up(1050, "1.1 KB")]
    #[case::almost_mb(999_999, "1000.0 KB")]
    #[case::mb(1_000_000, "1.0 MB")]
    #[case::almost_gb(999_999_999, "1000.0 MB")]
    #[case::gb(1_000_000_000, "1.0 GB")]
    #[case::almost_tb(999_999_999_999, "1000.0 GB")]
    #[case::tb(1_000_000_000_000, "1.0 TB")]
    fn test_format_byte_size(#[case] size: usize, #[case] expected: &str) {
        assert_eq!(&format_byte_size(size), expected);
    }
}
