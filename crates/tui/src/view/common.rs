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

use crate::view::{
    Generate,
    context::ViewContext,
    util::{format_duration, format_time},
};
use chrono::{DateTime, Duration, Utc};
use itertools::{Itertools, Position};
use ratatui::{
    prelude::{Buffer, Rect},
    symbols::merge::MergeStrategy,
    text::{Span, Text},
    widgets::{Block, Borders, Widget},
};
use reqwest::{StatusCode, header::HeaderValue};
use slumber_core::{collection::Profile, util::MaybeStr};
use std::{error::Error, ops::Deref, ptr};
use unicode_width::UnicodeWidthStr;

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
        let styles = ViewContext::styles().pane;
        let (border_type, border_style) = styles.border(self.has_focus);
        Block::default()
            .borders(Borders::ALL)
            .border_type(border_type)
            .border_style(border_style)
            .merge_borders(MergeStrategy::Fuzzy)
            .style(styles.generic)
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
        let styles = ViewContext::styles().status_code;
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
        = Text<'static>
    where
        Self: 'this;

    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this,
    {
        let mut text = Text::default().style(ViewContext::styles().text.error);
        // Walk down the error chain and build out a tree thing
        let mut next = Some(self);
        // How far in should the next error be indented? +1 per error
        let mut indent: usize = 0;

        // unstable: Use error.sources()
        // https://github.com/rust-lang/rust/issues/58520
        while let Some(error) = next {
            next = error.source();
            // First error doesn't get an icon
            let icon = if ptr::eq(self, error) {
                ""
            } else if next.is_some() {
                // If there's a following error, leave a little dangler
                "└┬"
            } else {
                "└─"
            };
            // Add additional indentation for account for the icon (-1 for the
            // dangler overlap). All lines for this error should start at the
            // same column (right of the icon). We don't want to accumulate this
            // width across errors though.
            let line_indent = (indent + icon.width()).saturating_sub(1);

            for (position, line) in error.to_string().lines().with_position() {
                // Show a different icon for continuation lines
                let icon = match position {
                    Position::First | Position::Only => icon,
                    // If there's an error after, extend the dangler
                    Position::Middle | Position::Last if next.is_some() => "│",
                    Position::Middle | Position::Last => "",
                };
                text.push_line(format!("{icon:>line_indent$}{line}"));
            }
            indent += 1;
        }

        text
    }
}

impl Generate for &anyhow::Error {
    /// 'static because string is generated
    type Output<'this>
        = Text<'static>
    where
        Self: 'this;

    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this,
    {
        (self.deref() as &dyn Error).generate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::test_util::{TestHarness, harness};
    use anyhow::anyhow;
    use rstest::rstest;

    /// Test error chain display
    ///
    /// - First error is displayed without indentation
    /// - Subsequent errors get a little tree guy with indentation
    /// - Continuation lines from a single error are indented as well
    #[rstest]
    fn test_error(_harness: TestHarness) {
        // Build the error inside-out
        let error = anyhow!("Third\nPoint at ! ^^\nthird line")
            .context("Second\nanother line!!")
            .context("First");
        let expected = "\
First
└┬Second
 │another line!!
 └─Third
   Point at ! ^^
   third line";
        let actual = error.generate().into_iter().join("\n");
        assert_eq!(actual, expected);
    }
}
