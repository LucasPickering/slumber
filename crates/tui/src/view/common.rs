//! Common reusable components for building the view. Children here should be
//! generic, i.e. usable in more than a single narrow context.

pub mod actions;
pub mod button;
pub mod component_select;
pub mod fixed_select;
pub mod header_table;
pub mod modal;
pub mod scrollbar;
pub mod select;
pub mod table;
pub mod tabs;
pub mod template_preview;
pub mod text_box;
pub mod text_window;

use crate::{
    context::TuiContext,
    view::{
        Generate,
        util::{format_duration, format_time},
    },
};
use chrono::{DateTime, Duration, Utc};
use itertools::{Itertools, Position};
use ratatui::{
    prelude::{Buffer, Rect},
    symbols::merge::MergeStrategy,
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Widget, Wrap},
};
use reqwest::{StatusCode, header::HeaderValue};
use slumber_core::{collection::Profile, util::MaybeStr};
use std::{error::Error, ops::Deref, ptr};

/// A container with a title and border
pub struct Pane<'a> {
    pub title: &'a str,
    pub has_focus: bool,
}

impl Generate for Pane<'_> {
    type Output<'this>
        = Block<'this>
    where
        Self: 'this;

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
            .merge_borders(MergeStrategy::Fuzzy)
            .title(self.title)
    }
}

/// Yes or no?
pub struct Checkbox {
    pub checked: bool,
}

impl Widget for Checkbox {
    fn render(self, area: Rect, buf: &mut Buffer)
    where
        Self: Sized,
    {
        let s = if self.checked { "[x]" } else { "[ ]" };
        s.render(area, buf);
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
    type Output<'this>
        = Text<'this>
    where
        Self: 'this;

    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this,
    {
        self.as_str().into()
    }
}

impl Generate for &Profile {
    type Output<'this>
        = Span<'this>
    where
        Self: 'this;

    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this,
    {
        self.name().to_owned().into()
    }
}

/// Format a timestamp in the local timezone
impl Generate for DateTime<Utc> {
    type Output<'this>
        = Span<'this>
    where
        Self: 'this;

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
    type Output<'this>
        = Span<'this>
    where
        Self: 'this;

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
    type Output<'this>
        = Span<'this>
    where
        Self: 'this;

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
    type Output<'this>
        = Span<'this>
    where
        Self: 'this;

    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this,
    {
        MaybeStr(self.as_bytes()).to_string().into()
    }
}

// Render the error chain. This can't be a blanket impl on `E: Error` because
// that generates potential "conflicts"
impl Generate for &dyn Error {
    /// 'static because string is generated
    type Output<'this>
        = Paragraph<'static>
    where
        Self: 'this;

    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this,
    {
        let mut lines: Vec<Line> = Vec::new();
        // Walk down the error chain and build out a tree thing
        let mut next = Some(self);
        // unstable: Use error.sources()
        // https://github.com/rust-lang/rust/issues/58520
        while let Some(error) = next {
            next = error.source();
            let icon = if ptr::eq(self, error) {
                // This is the first in the chain
                ""
            } else if next.is_some() {
                "└┬"
            } else {
                "└─"
            };
            for (position, line) in error.to_string().lines().with_position() {
                let line = if let Position::First | Position::Only = position {
                    format!(
                        "{indent:width$}{icon}{line}",
                        indent = "",
                        width = lines.len().saturating_sub(1)
                    )
                } else {
                    line.to_owned()
                };
                lines.push(line.into());
            }
        }
        Paragraph::new(lines).wrap(Wrap::default())
    }
}

impl Generate for &anyhow::Error {
    /// 'static because string is generated
    type Output<'this>
        = Paragraph<'static>
    where
        Self: 'this;

    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this,
    {
        (self.deref() as &dyn Error).generate()
    }
}
