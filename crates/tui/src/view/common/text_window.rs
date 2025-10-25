use crate::{
    context::TuiContext,
    view::{
        common::scrollbar::Scrollbar,
        component::{Canvas, Component, ComponentId, Draw, DrawMetadata},
        context::UpdateContext,
        event::{Event, OptionEvent},
        state::{Identified, StateCell},
    },
};
use ratatui::{
    buffer::Buffer,
    layout::{Layout, Rect},
    prelude::{Alignment, Constraint},
    text::{Line, StyledGrapheme, Text},
    widgets::{Paragraph, ScrollbarOrientation},
};
use slumber_config::Action;
use std::{cell::Cell, cmp};
use unicode_width::UnicodeWidthStr;
use uuid::Uuid;

/// A scrollable (but not editable) block of text. Internal state will be
/// updated on each render, to adjust to the text's width/height. Generally the
/// parent should be storing an instance of [Text] and passing the same value to
/// this on each render. Generating the `Text` could potentially be expensive
/// (especially if it includes syntax highlighting).
#[derive(derive_more::Debug, Default)]
pub struct TextWindow {
    id: ComponentId,
    /// Cache the size of the text window, because it's expensive to calculate.
    /// Checking the width of a text requires counting all its graphemes.
    text_size: StateCell<Uuid, TextSize>,
    /// Horizontal scroll
    offset_x: Cell<usize>,
    /// Vertical scroll
    offset_y: Cell<usize>,
    /// How wide is the visible text area, excluding gutter/scrollbars?
    window_width: Cell<usize>,
    /// How tall is the visible text area, excluding gutter/scrollbars?
    window_height: Cell<usize>,
}

#[derive(Clone, Debug)]
pub struct TextWindowProps<'a> {
    /// Text to render. We take a reference because this component tends to
    /// contain a lot of text, and we don't want to force a clone on render
    pub text: &'a Identified<Text<'a>>,
    pub margins: ScrollbarMargins,
}

/// How far outside the text window should scrollbars be placed? Margin of
/// 0 uses the outermost row/column of the text area. Positive values
/// pushes the scrollbar outside the rendered outside, negative moves
/// it inside.
#[derive(Clone, Debug)]
pub struct ScrollbarMargins {
    pub right: i32,
    pub bottom: i32,
}

impl Default for ScrollbarMargins {
    fn default() -> Self {
        Self {
            right: 1,
            bottom: 1,
        }
    }
}

#[derive(Debug, Default)]
struct TextSize {
    /// Number of graphemes in the longest line in the text
    width: usize,
    /// Number of lines in the text
    height: usize,
}

impl TextWindow {
    /// Get the final line that we can't scroll past. This will be the first
    /// line of the last page of text
    fn max_scroll_line(&self) -> usize {
        let text_height = self.text_size.borrow().height;
        text_height.saturating_sub(self.window_height.get())
    }

    /// Get the final column that we can't scroll (horizontally) past. This will
    /// be the left edge of the rightmost "page" of text
    fn max_scroll_column(&self) -> usize {
        let text_width = self.text_size.borrow().width;
        text_width.saturating_sub(self.window_width.get())
    }

    fn scroll_up(&mut self, lines: usize) {
        *self.offset_y.get_mut() = self.offset_y.get().saturating_sub(lines);
    }

    fn scroll_down(&mut self, lines: usize) {
        *self.offset_y.get_mut() =
            cmp::min(self.offset_y.get() + lines, self.max_scroll_line());
    }

    /// Scroll to a specific line number. The target line will end up as close
    /// to the top of the page as possible
    fn scroll_to(&mut self, line: usize) {
        *self.offset_y.get_mut() = cmp::min(line, self.max_scroll_line());
    }

    fn scroll_left(&mut self, columns: usize) {
        *self.offset_x.get_mut() = self.offset_x.get().saturating_sub(columns);
    }

    fn scroll_right(&mut self, columns: usize) {
        *self.offset_x.get_mut() =
            cmp::min(self.offset_x.get() + columns, self.max_scroll_column());
    }

    /// Ensure the scroll state is valid. Called on every render, in case the
    /// text size or draw area changed
    fn clamp_scroll(&self) {
        self.offset_x
            .set(cmp::min(self.offset_x.get(), self.max_scroll_column()));
        self.offset_y
            .set(cmp::min(self.offset_y.get(), self.max_scroll_line()));
    }

    /// Render the visible text into the window. The Paragraph widget provides
    /// all this functionality out of the box, but it needs an owned Text and
    /// we only have a reference. A clone could potentially be very expensive
    /// for a large body, so we use our own logic.
    fn render_chars<'a>(
        &self,
        text: &'a Text<'a>,
        buf: &mut Buffer,
        area: Rect,
    ) {
        let lines = text
            .lines
            .iter()
            .skip(self.offset_y.get())
            .take(self.window_height.get())
            .enumerate();
        for (y, line) in lines {
            // This could be expensive if we're skipping a lot of graphemes,
            // i.e. scrolled far to the right in a wide body. Fortunately that's
            // a niche use case so not optimized for yet. To fix this we would
            // have to map grapheme number -> byte offset and cache that,
            // because skipping bytes is O(1) instead of O(n)
            let graphemes = line
                .styled_graphemes(text.style)
                .skip(self.offset_x.get())
                .take(self.window_width.get());
            let mut x = 0;
            for StyledGrapheme { symbol, style } in graphemes {
                if x >= area.width {
                    break;
                }
                buf[(area.left() + x, area.top() + y as u16)]
                    .set_symbol(symbol)
                    .set_style(style);
                x += symbol.width() as u16;
            }
        }
    }
}

impl Component for TextWindow {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn update(&mut self, _: &mut UpdateContext, event: Event) -> Option<Event> {
        event.opt().action(|action, propagate| match action {
            Action::Up | Action::ScrollUp => self.scroll_up(1),
            Action::Down | Action::ScrollDown => self.scroll_down(1),
            Action::ScrollLeft => self.scroll_left(1),
            Action::ScrollRight => self.scroll_right(1),
            Action::PageUp => self.scroll_up(self.window_height.get()),
            Action::PageDown => self.scroll_down(self.window_height.get()),
            Action::Home => self.scroll_to(0),
            Action::End => self.scroll_to(usize::MAX),
            _ => propagate.set(),
        })
    }
}

/// `T` has to be convertible to text to be drawn
impl<'a> Draw<TextWindowProps<'a>> for TextWindow {
    fn draw_impl(
        &self,
        canvas: &mut Canvas,
        props: TextWindowProps<'a>,
        metadata: DrawMetadata,
    ) {
        let styles = &TuiContext::get().styles;

        let text_state = self.text_size.get_or_update(&props.text.id(), || {
            // Note: Paragraph has methods for this, but that requires an
            // owned copy of Text, which involves a lot of cloning

            // This counts _graphemes_, not bytes, so it's O(n)
            let text_width = props
                .text
                .lines
                .iter()
                .map(Line::width)
                .max()
                .unwrap_or_default();
            // Assume no line wrapping when calculating line count
            let text_height = props.text.lines.len();
            TextSize {
                width: text_width,
                height: text_height,
            }
        });

        let [gutter_area, _, text_area] = Layout::horizontal([
            // Size gutter based on width of max line number
            Constraint::Length(
                (text_state.height as f32).log10().floor() as u16 + 1,
            ),
            Constraint::Length(1), // Spacer
            Constraint::Min(0),
        ])
        .areas(metadata.area());
        let has_vertical_scroll = text_state.height > text_area.height as usize;
        let has_horizontal_scroll = text_state.width > text_area.width as usize;

        // Store text and window sizes for calculations in the update code
        self.window_width.set(text_area.width as usize);
        self.window_height.set(text_area.height as usize);

        // Scroll state could become invalid if window size or text changes
        self.clamp_scroll();

        // Draw line numbers in the gutter
        let first_line = self.offset_y.get() + 1;
        let last_line = cmp::min(
            first_line + self.window_height.get() - 1,
            text_state.height,
        );
        canvas.render_widget(
            Paragraph::new(
                (first_line..=last_line)
                    .map(|n| n.to_string().into())
                    .collect::<Vec<Line>>(),
            )
            .alignment(Alignment::Right)
            .style(styles.text_window.gutter),
            gutter_area,
        );

        // Draw the text content
        self.render_chars(props.text, canvas.buffer_mut(), text_area);

        // Scrollbars
        if has_vertical_scroll {
            canvas.render_widget(
                Scrollbar {
                    content_length: text_state.height,
                    offset: self.offset_y.get(),
                    // We subtracted the margin from the text area before, so
                    // we have to add that back now
                    margin: props.margins.right,
                    ..Default::default()
                },
                text_area,
            );
        }
        if has_horizontal_scroll {
            canvas.render_widget(
                Scrollbar {
                    content_length: text_state.width,
                    offset: self.offset_x.get(),
                    orientation: ScrollbarOrientation::HorizontalBottom,
                    // See note on other scrollbar for +1
                    margin: props.margins.bottom,
                },
                text_area,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_util::{TestHarness, TestTerminal, harness, terminal},
        view::test_util::TestComponent,
    };
    use ratatui::text::Span;
    use rstest::rstest;
    use terminput::{KeyCode, KeyModifiers};

    #[rstest]
    fn test_scroll(
        #[with(10, 4)] terminal: TestTerminal,
        harness: TestHarness,
    ) {
        let text =
            Text::from("line 1\nline 2 is longer\nline 3\nline 4\nline 5")
                .into();
        let props = TextWindowProps {
            text: &text,
            // Don't overflow the frame
            margins: ScrollbarMargins {
                right: 0,
                bottom: 0,
            },
        };
        let mut component =
            TestComponent::builder(&harness, &terminal, TextWindow::default())
                .with_props(props.clone())
                .build();
        terminal.assert_buffer_lines([
            vec![line_num(1), " line 1 ▲".into()],
            vec![line_num(2), " line 2 █".into()],
            vec![line_num(3), " line 3 █".into()],
            vec![line_num(4), " ◀■■■═══▶".into()],
        ]);

        // Scroll down
        component
            .int_props(|| props.clone())
            .send_key(KeyCode::Down)
            .assert_empty();
        terminal.assert_buffer_lines([
            vec![line_num(2), " line 2 ▲".into()],
            vec![line_num(3), " line 3 █".into()],
            vec![line_num(4), " line 4 █".into()],
            vec![line_num(5), " ◀■■■═══▶".into()],
        ]);

        // Scroll back up
        component
            .int_props(|| props.clone())
            // Second does nothing
            .send_keys([KeyCode::Up, KeyCode::Up])
            .assert_empty();
        terminal.assert_buffer_lines([
            vec![line_num(1), " line 1 ▲".into()],
            vec![line_num(2), " line 2 █".into()],
            vec![line_num(3), " line 3 █".into()],
            vec![line_num(4), " ◀■■■═══▶".into()],
        ]);

        // Scroll right
        component
            .int_props(|| props.clone())
            .send_key_modifiers(KeyCode::Right, KeyModifiers::SHIFT)
            .send_key_modifiers(KeyCode::Right, KeyModifiers::SHIFT)
            .send_key_modifiers(KeyCode::Right, KeyModifiers::SHIFT)
            .assert_empty();
        terminal.assert_buffer_lines([
            vec![line_num(1), " e 1    ▲".into()],
            vec![line_num(2), " e 2 is █".into()],
            vec![line_num(3), " e 3    █".into()],
            vec![line_num(4), " ◀═■■■══▶".into()],
        ]);

        // Scroll back left
        component
            .int_props(|| props.clone())
            .send_key_modifiers(KeyCode::Left, KeyModifiers::SHIFT)
            .send_key_modifiers(KeyCode::Left, KeyModifiers::SHIFT)
            .send_key_modifiers(KeyCode::Left, KeyModifiers::SHIFT)
            // Does nothing
            .send_key_modifiers(KeyCode::Left, KeyModifiers::SHIFT)
            .assert_empty();
        terminal.assert_buffer_lines([
            vec![line_num(1), " line 1 ▲".into()],
            vec![line_num(2), " line 2 █".into()],
            vec![line_num(3), " line 3 █".into()],
            vec![line_num(4), " ◀■■■═══▶".into()],
        ]);
    }

    #[rstest]
    fn test_unicode(
        #[with(35, 3)] terminal: TestTerminal,
        harness: TestHarness,
    ) {
        let text =
            Text::from("intro\n💚💙💜 this is a longer line\noutro").into();
        TestComponent::builder(&harness, &terminal, TextWindow::default())
            .with_props(TextWindowProps {
                text: &text,
                // Don't overflow the frame
                margins: ScrollbarMargins {
                    right: 0,
                    bottom: 0,
                },
            })
            .build();
        terminal.assert_buffer_lines([
            vec![line_num(1), " intro                            ".into()],
            vec![line_num(2), " 💚💙💜 this is a longer line    ".into()],
            vec![line_num(3), " outro                            ".into()],
        ]);
    }

    #[rstest]
    fn test_unicode_scroll(
        #[with(10, 2)] terminal: TestTerminal,
        harness: TestHarness,
    ) {
        let text = Text::raw("💚💙💜💚💙💜").into();
        TestComponent::builder(&harness, &terminal, TextWindow::default())
            .with_props(TextWindowProps {
                text: &text,
                // Don't overflow the frame
                margins: ScrollbarMargins {
                    right: 0,
                    bottom: 0,
                },
            })
            .build();
        terminal.assert_buffer_lines([
            vec![line_num(1), " 💚💙💜💚".into()],
            vec![line_num(0), " ◀■■■■══▶".into()],
        ]);
    }

    /// Shrinking text reduces the maximum scroll. Scroll state should
    /// automatically be clamped to match
    #[rstest]
    fn test_shrink_text(
        #[with(10, 3)] terminal: TestTerminal,
        harness: TestHarness,
    ) {
        let text =
            Text::from_iter(["1 this is a long line", "2", "3", "4", "5"])
                .into();
        let mut component =
            TestComponent::builder(&harness, &terminal, TextWindow::default())
                .with_props(TextWindowProps {
                    text: &text,
                    // Don't overflow the frame
                    margins: ScrollbarMargins {
                        right: 0,
                        bottom: 0,
                    },
                })
                .build();

        // Scroll out a bit
        component.scroll_down(2);
        component.scroll_right(10);
        assert_eq!(component.offset_x.get(), 10);
        assert_eq!(component.offset_y.get(), 2);

        let text = Text::from_iter(["1 less long line", "2", "3", "4"]).into();
        component
            .int_props(|| TextWindowProps {
                text: &text,
                margins: ScrollbarMargins {
                    right: 0,
                    bottom: 0,
                },
            })
            .drain_draw()
            .assert_empty();

        assert_eq!(component.offset_x.get(), 8);
        assert_eq!(component.offset_y.get(), 1);
    }

    /// Growing the window reduces the maximum scroll. Scroll state should
    /// automatically be clamped to match
    #[rstest]
    fn test_grow_window(terminal: TestTerminal, harness: TestHarness) {
        let text =
            Text::from_iter(["1 this is a long line", "2", "3", "4", "5"])
                .into();
        let props = TextWindowProps {
            text: &text,
            // Don't overflow the frame
            margins: ScrollbarMargins {
                right: 0,
                bottom: 0,
            },
        };
        let mut component =
            TestComponent::builder(&harness, &terminal, TextWindow::default())
                .with_props(props.clone())
                .build();

        component.set_area(Rect::new(0, 0, 10, 3));
        component
            .int_props(|| props.clone())
            .drain_draw()
            .assert_empty();

        // Scroll out a bit
        component.scroll_down(2);
        component.scroll_right(10);
        assert_eq!(component.offset_x.get(), 10);
        assert_eq!(component.offset_y.get(), 2);

        component.set_area(Rect::new(0, 0, 15, 4));
        component
            .int_props(|| props.clone())
            .drain_draw()
            .assert_empty();

        assert_eq!(component.offset_x.get(), 8);
        assert_eq!(component.offset_y.get(), 1);
    }

    /// Style some text as gutter line numbers
    fn line_num(n: u16) -> Span<'static> {
        let s = if n > 0 { n.to_string() } else { " ".into() };
        Span::styled(s, TuiContext::get().styles.text_window.gutter)
    }
}
