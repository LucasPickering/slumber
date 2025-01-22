//! Helper structs and functions for building components

pub mod highlight;
pub mod persistence;

use crate::{
    message::Message,
    util::{spawn, temp_file},
    view::ViewContext,
};
use anyhow::Context;
use itertools::Itertools;
use mime::Mime;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    text::{Line, Text},
};
use slumber_core::{
    template::{Prompt, PromptChannel, Prompter, Select},
    util::ResultTraced,
};
use std::{io::Write, path::Path, time::Duration};
use tokio::{task::AbortHandle, time};

/// A data structure for representation a yes/no confirmation. This is similar
/// to [Prompt], but it only asks a yes/no question.
#[derive(Debug)]
pub struct Confirm {
    /// Question to ask the user
    pub message: String,
    /// A channel to pass back the user's response
    pub channel: PromptChannel<bool>,
}

/// A prompter that returns a static value; used for template previews, where
/// user interaction isn't possible
#[derive(Debug)]
pub struct PreviewPrompter;

impl Prompter for PreviewPrompter {
    fn prompt(&self, prompt: Prompt) {
        prompt.channel.respond("<prompt>".into())
    }

    fn select(&self, select: Select) {
        select.channel.respond("<select>".into())
    }
}

/// Utility for debouncing repeated calls to a callback
#[derive(Debug)]
pub struct Debounce {
    duration: Duration,
    abort_handle: Option<AbortHandle>,
}

impl Debounce {
    pub fn new(duration: Duration) -> Self {
        Self {
            duration,
            abort_handle: None,
        }
    }

    /// Trigger a debounced callback. The given callback will be invoked after
    /// the debounce period _if_ this method is not called again during the
    /// debounce period.
    pub fn start(&mut self, on_complete: impl 'static + Fn()) {
        // Cancel the existing debounce, if any
        self.cancel();

        // Run debounce in a local task so component behavior can access the
        // view context, e.g. to push events
        let duration = self.duration;
        let handle = spawn("Debounce", async move {
            time::sleep(duration).await;
            on_complete();
        });
        self.abort_handle = Some(handle.abort_handle());
    }

    /// Cancel the current pending callback (if any) without registering a new
    /// one
    pub fn cancel(&mut self) {
        if let Some(abort_handle) = self.abort_handle.take() {
            abort_handle.abort();
        }
    }
}

/// Created a rectangle centered on the given `Rect`.
pub fn centered_rect(
    width: Constraint,
    height: Constraint,
    rect: Rect,
) -> Rect {
    fn buffer(constraint: Constraint, full_size: u16) -> Constraint {
        match constraint {
            Constraint::Percentage(percent) => {
                Constraint::Percentage((100 - percent) / 2)
            }
            Constraint::Length(length) => {
                Constraint::Length((full_size.saturating_sub(length)) / 2)
            }
            // Implement these as needed
            _ => unimplemented!("Other center constraints unsupported"),
        }
    }

    let buffer_x = buffer(width, rect.width);
    let buffer_y = buffer(height, rect.height);
    let columns = Layout::default()
        .direction(Direction::Vertical)
        .constraints([buffer_y, height, buffer_y].as_ref())
        .split(rect);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([buffer_x, width, buffer_x].as_ref())
        .split(columns[1])[1]
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
    // Shitty try block
    fn helper(text: &Text, path: &Path) -> anyhow::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path)?;
        for line in &text.lines {
            for span in &line.spans {
                file.write_all(span.content.as_bytes())?;
            }
            // Every line gets a line ending, so we end up with a trailing one
            file.write_all(b"\n")?;
        }
        Ok(())
    }

    let path = temp_file();
    let result = helper(text, &path)
        .with_context(|| format!("Error writing to file {path:?}"))
        .traced();
    match result {
        Ok(()) => ViewContext::send_message(Message::FileView { path, mime }),
        Err(error) => ViewContext::send_message(Message::Error { error }),
    }
}
