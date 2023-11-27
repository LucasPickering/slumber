use crate::{
    collection::ProfileId,
    template::{Template, TemplateChunk},
    tui::{context::TuiContext, message::Message, view::draw::Generate},
};
use derive_more::Deref;
use ratatui::{
    buffer::Buffer,
    prelude::Rect,
    style::Style,
    text::{Line, Span, Text},
    widgets::{Paragraph, Widget},
};
use std::{
    mem,
    sync::{Arc, OnceLock},
};

/// A preview of a template string, which can show either the raw text or the
/// rendered version. This switch is stored in render context, so it can be
/// changed globally.
#[derive(Debug)]
pub enum TemplatePreview {
    /// Template previewing is disabled, just show the raw text
    Disabled { template: Template },
    /// Template previewing is enabled, render the template
    Enabled {
        template: Template,
        /// Rendered areas. On init we send a message which will trigger a
        /// task to start the render. When the task is done, it'll dump
        /// its result back here.
        areas: Arc<OnceLock<Vec<TemplateChunk>>>,
    },
}

impl TemplatePreview {
    /// Create a new template preview. This will spawn a background task to
    /// render the template. Profile ID defines which profile to use for the
    /// render.
    pub fn new(
        template: Template,
        profile_id: Option<ProfileId>,
        enabled: bool,
    ) -> Self {
        if enabled {
            // Tell the controller to start rendering the preview, and it'll
            // store it back here when done
            let lock = Arc::new(OnceLock::new());
            TuiContext::send_message(Message::TemplatePreview {
                // If this is a bottleneck we can Arc it
                template: template.clone(),
                profile_id,
                destination: Arc::clone(&lock),
            });

            Self::Enabled {
                template,
                areas: lock,
            }
        } else {
            Self::Disabled { template }
        }
    }
}

impl Generate for &TemplatePreview {
    type Output<'this> = Text<'this>
    where
        Self: 'this;

    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this,
    {
        // The raw template string
        match self {
            TemplatePreview::Disabled { template } => template.deref().into(),
            TemplatePreview::Enabled { template, areas } => {
                // If the preview render is ready, show it. Otherwise fall back
                // to the raw
                match areas.get() {
                    Some(areas) => TextStitcher::stitch_chunks(template, areas),
                    // Preview still rendering
                    None => template.deref().into(),
                }
            }
        }
    }
}

impl Widget for &TemplatePreview {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let text = self.generate();
        Paragraph::new(text).render(area, buf)
    }
}

/// A helper for stitching rendered template areas into ratatui `Text`. This
/// requires some effort because ratatui *loves* line breaks, so we have to
/// very manually construct the text to make sure the structure reflects the
/// line breaks in the input.
///
/// See ratatui docs: https://docs.rs/ratatui/latest/ratatui/text/index.html
#[derive(Debug, Default)]
struct TextStitcher<'a> {
    completed_lines: Vec<Line<'a>>,
    next_line: Vec<Span<'a>>,
}

impl<'a> TextStitcher<'a> {
    /// Convert areas into a series of spans, which can be turned into a line
    fn stitch_chunks(
        template: &'a Template,
        areas: &'a [TemplateChunk],
    ) -> Text<'a> {
        let theme = &TuiContext::get().theme;

        // Each area will get its own styling, but we can't just make each
        // area a Span, because one area might have multiple lines. And we
        // can't make each area a Line, because multiple areas might be
        // together on the same line. So we need to walk down each line and
        // manually split the lines
        let mut stitcher = Self::default();
        for area in areas {
            let (area_text, style) = match &area {
                TemplateChunk::Raw(span) => {
                    (template.substring(*span), Style::default())
                }
                TemplateChunk::Rendered { value, sensitive } => {
                    let value = if *sensitive {
                        // Hide sensitive values. Ratatui has a Masked type, but
                        // it complicates the string ownership a lot and also
                        // exposes the length of the sensitive text
                        "<sensitive>"
                    } else {
                        value.as_str()
                    };
                    (value, theme.template_preview_text)
                }
                // There's no good way to render the entire error inline
                TemplateChunk::Error(_) => {
                    ("Error", theme.template_preview_error)
                }
            };

            stitcher.add_area(area_text, style);
        }
        stitcher.into_text()
    }

    /// Add one area to the text. This will recursively split on any line
    /// breaks in the text until it reaches the end.
    fn add_area(&mut self, area_text: &'a str, style: Style) {
        // If we've reached a line ending, push the line and start a new one.
        // Intentionally ignore \r; it won't cause any harm in the output text
        match area_text.split_once('\n') {
            Some((a, b)) => {
                self.add_span(a, style);
                self.end_line();
                // Recursion!
                self.add_area(b, style);
            }
            // This area has no line breaks, just add it and move on
            None => self.add_span(area_text, style),
        }
    }

    fn add_span(&mut self, text: &'a str, style: Style) {
        if !text.is_empty() {
            self.next_line.push(Span::styled(text, style));
        }
    }

    /// Add the current line to the accumulator, and start a new one
    fn end_line(&mut self) {
        if !self.next_line.is_empty() {
            self.completed_lines
                .push(mem::take(&mut self.next_line).into());
        }
    }

    /// Convert all lines into a text block
    fn into_text(mut self) -> Text<'a> {
        self.end_line(); // Make sure to include whatever wasn't finished
        Text::from(self.completed_lines)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::factory::*;
    use factori::create;
    use indexmap::indexmap;

    /// Test these cases related to line breaks:
    /// - Line break within a raw area
    /// - Line break within a rendered area
    /// - Line break at area boundary
    /// - NO line break at area boundary
    /// Ratatui is fucky with how it handles line breaks in text, so we need
    /// to make sure our output reflects the input
    ///
    /// Additionally, test multi-byte unicode characters to make sure string
    /// offset indexes work correctly
    #[tokio::test]
    async fn test_template_stitch() {
        // Render a template
        let template = Template::parse(
            "intro\n{{user_id}} 💚💙💜 {{unknown}}\noutro\r\nmore outro".into(),
        )
        .unwrap();
        let profile = indexmap! { "user_id".into() => "🧡\n💛".into() };
        let context = create!(TemplateContext, profile: profile);
        let areas = template.render_chunks(&context).await;
        TuiContext::init_test();
        let theme = &TuiContext::get().theme;

        let text = TextStitcher::stitch_chunks(&template, &areas);
        let rendered_style = theme.template_preview_text;
        let error_style = theme.template_preview_error;
        let expected = Text::from(vec![
            Line::from("intro"),
            Line::from(Span::styled("🧡", rendered_style)),
            Line::from(vec![
                Span::styled("💛", rendered_style),
                Span::raw(" 💚💙💜 "),
                Span::styled("Error", error_style),
            ]),
            Line::from("outro\r"), // \r shouldn't create any issues
            Line::from("more outro"),
        ]);
        assert_eq!(text, expected);
    }
}
