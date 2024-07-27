//! Common reusable components for building the view. Children here should be
//! generic, i.e. usable in more than a single narrow context.

pub mod actions;
pub mod button;
pub mod header_table;
pub mod list;
pub mod modal;
pub mod scrollbar;
pub mod table;
pub mod tabs;
pub mod template_preview;
pub mod text_box;
pub mod text_window;

use crate::tui::{
    context::TuiContext,
    view::{draw::Generate, state::Notification},
};
use chrono::{DateTime, Duration, Local, Utc};
use itertools::Itertools;
use ratatui::{
    text::{Line, Span, Text},
    widgets::{Block, Borders},
};
use reqwest::{header::HeaderValue, StatusCode};
use slumber_core::{
    collection::Profile,
    http::{RequestBuildError, RequestError},
    util::{format_duration, format_time, MaybeStr},
};

/// A container with a title and border
pub struct Pane<'a> {
    pub title: &'a str,
    pub has_focus: bool,
}

impl<'a> Generate for Pane<'a> {
    type Output<'this> = Block<'this> where Self: 'this;

    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this,
    {
        let (border_type, border_style) =
            TuiContext::get().styles.pane.border(self.has_focus);
        Block::default()
            .borders(Borders::ALL)
            .border_type(border_type)
            .border_style(border_style)
            .title(self.title)
    }
}

/// Yes or no?
pub struct Checkbox {
    pub checked: bool,
}

impl Generate for Checkbox {
    type Output<'this> = Text<'static>;

    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this,
    {
        if self.checked {
            "[x]".into()
        } else {
            "[ ]".into()
        }
    }
}

impl Generate for String {
    /// Use `Text` because a string can be multiple lines
    type Output<'this> = Text<'this>;

    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this,
    {
        self.into()
    }
}

impl Generate for &String {
    /// Use `Text` because a string can be multiple lines
    type Output<'this> = Text<'this> where Self: 'this;

    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this,
    {
        self.as_str().into()
    }
}

impl Generate for &Profile {
    type Output<'this> = Span<'this> where Self: 'this;

    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this,
    {
        self.name().to_owned().into()
    }
}

impl Generate for &Notification {
    type Output<'this> = Span<'this> where Self: 'this;

    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this,
    {
        format!(
            "[{}] {}",
            self.timestamp.with_timezone(&Local).format("%H:%M:%S"),
            self.message
        )
        .into()
    }
}

/// Format a timestamp in the local timezone
impl Generate for DateTime<Utc> {
    type Output<'this> = Span<'this> where Self: 'this;

    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this,
    {
        format_time(&self).to_string().into()
    }
}

impl Generate for Duration {
    /// 'static because string is generated
    type Output<'this> = Span<'static>;

    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this,
    {
        format_duration(&self).into()
    }
}

impl Generate for Option<Duration> {
    type Output<'this> = Span<'this> where Self: 'this;

    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this,
    {
        match self {
            Some(duration) => duration.generate(),
            // For incomplete requests typically
            None => "???".into(),
        }
    }
}

impl Generate for StatusCode {
    type Output<'this> = Span<'this> where Self: 'this;

    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this,
    {
        let styles = &TuiContext::get().styles.status_code;
        let is_error = self.is_client_error() || self.is_server_error();
        Span::styled(
            self.to_string(),
            if is_error {
                styles.error
            } else {
                styles.success
            },
        )
    }
}

/// Not all header values are UTF-8; use a placeholder if not
impl Generate for &HeaderValue {
    type Output<'this> = Span<'this> where Self: 'this;

    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this,
    {
        MaybeStr(self.as_bytes()).to_string().into()
    }
}

impl Generate for &anyhow::Error {
    /// 'static because string is generated
    type Output<'this> = Text<'static> where Self: 'this;

    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this,
    {
        let chain = self.chain();
        let len = chain.len();
        chain
            .enumerate()
            .map::<Line, _>(|(i, error)| {
                let icon = if i == 0 {
                    "" // First
                } else if i < len - 1 {
                    "└┬" // Intermediate
                } else {
                    "└─" // Last
                };
                format!(
                    "{indent:width$}{icon}{error}",
                    indent = "",
                    width = i.saturating_sub(1)
                )
                .into()
            })
            .collect_vec()
            .into()
    }
}

impl Generate for &RequestBuildError {
    type Output<'this> = Text<'static> where Self: 'this;

    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this,
    {
        // Defer to the underlying anyhow error
        self.error.generate()
    }
}

impl Generate for &RequestError {
    type Output<'this> = Text<'static> where Self: 'this;

    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this,
    {
        // Defer to the underlying anyhow error
        self.error.generate()
    }
}
