use crate::view::{
    common::scrollbar::Scrollbar,
    component::{Canvas, Component, ComponentId, Draw, DrawMetadata},
    context::{UpdateContext, ViewContext},
    event::{Event, EventMatch},
};
use ratatui::{
    buffer::Buffer,
    layout::{Layout, Rect, Size},
    prelude::{Alignment, Constraint},
    text::{Line, StyledGrapheme, Text},
    widgets::{ScrollbarOrientation, Widget},
};
use slumber_config::Action;
use std::{cell::Cell, cmp};
use terminput::ScrollDirection;
use unicode_width::UnicodeWidthStr;

/// A scrollable (but not editable) block of text
///
/// The displayed text is immutable; if the text changes, a new `TextWindow`
/// must be created
#[derive(Debug)]
pub struct TextWindow {
    id: ComponentId,
    /// Rendered text. Only the visible subset of this is drawn to the screen
    text: Text<'static>,
    /// `(width, height)` of the rendered text, in terms of lines/columns. This
    /// is computed at init because counting columns can be expensive due to
    /// multibyte UTF-8 chars
    text_size: TextSize,
    /// Size of the space we last rendered to (excluding gutter/scrollbars).
    /// Updated on each `draw()` call.
    window_size: Cell<Size>,
    /// `(horizontal, vertical)` scroll. In a `Cell` because it may be clamped
    /// if the window size changes
    offset: Cell<Offset>,
}

impl TextWindow {
    pub fn new(text: Text<'static>) -> Self {
        let text_size = TextSize::new(&text);

        Self {
            id: ComponentId::new(),
            text,
            text_size,
            window_size: Default::default(),
            offset: Default::default(),
        }
    }

    /// Get the full text
    pub fn text(&self) -> &Text<'static> {
        &self.text
    }

    /// Get the final line that we can't scroll past. This will be the first
    /// line of the last page of text
    fn max_scroll_line(&self) -> usize {
        let text_height = self.text_size.height;
        let window_height = self.window_size.get().height as usize;
        text_height.saturating_sub(window_height)
    }

    /// Get the final column that we can't scroll (horizontally) past. This will
    /// be the left edge of the rightmost "page" of text
    fn max_scroll_column(&self) -> usize {
        let text_width = self.text_size.width;
        let window_width = self.window_size.get().width as usize;
        text_width.saturating_sub(window_width)
    }

    fn scroll_up(&mut self, lines: usize) {
        self.offset.get_mut().y = self.offset.get().y.saturating_sub(lines);
    }

    fn scroll_down(&mut self, lines: usize) {
        self.offset.get_mut().y =
            cmp::min(self.offset.get().y + lines, self.max_scroll_line());
    }

    /// Scroll to a specific line number. The target line will end up as close
    /// to the top of the page as possible
    fn scroll_to(&mut self, line: usize) {
        self.offset.get_mut().y = cmp::min(line, self.max_scroll_line());
    }

    fn scroll_left(&mut self, columns: usize) {
        self.offset.get_mut().x = self.offset.get().x.saturating_sub(columns);
    }

    fn scroll_right(&mut self, columns: usize) {
        self.offset.get_mut().x =
            cmp::min(self.offset.get().x + columns, self.max_scroll_column());
    }

    /// Ensure the scroll state is valid. Called on every render, in case the
    /// text size or draw area changed
    fn clamp_scroll(&self) {
        let offset = self.offset.get();
        self.offset.set(Offset {
            x: cmp::min(offset.x, self.max_scroll_column()),
            y: cmp::min(offset.y, self.max_scroll_line()),
        });
    }

    /// Render the visible text into the window. The Paragraph widget provides
    /// all this functionality out of the box, but it needs an owned Text and
    /// we only have a reference. A clone could potentially be very expensive
    /// for a large body, so we use our own logic.
    fn render_text(&self, buf: &mut Buffer, area: Rect) {
        let offset = self.offset.get();
        let window_size = self.window_size.get();
        let lines = self
            .text
            .lines
            .iter()
            .skip(offset.y)
            .take(window_size.height.into())
            .enumerate();
        for (y, line) in lines {
            // This could be expensive if we're skipping a lot of graphemes,
            // i.e. scrolled far to the right in a wide body. Fortunately that's
            // a niche use case so not optimized for yet. To fix this we would
            // have to map grapheme number -> byte offset and cache that,
            // because skipping bytes is O(1) instead of O(n)
            let graphemes = line
                .styled_graphemes(self.text.style)
                .skip(offset.x)
                .take(window_size.width.into());
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

impl Default for TextWindow {
    fn default() -> Self {
        Self::new(Text::default())
    }
}

impl Component for TextWindow {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn update(&mut self, _: &mut UpdateContext, event: Event) -> EventMatch {
        event
            .m()
            // Scroll for scroll wheel OR keyboard inputs
            .scroll(|direction| match direction {
                ScrollDirection::Up => self.scroll_up(1),
                ScrollDirection::Down => self.scroll_down(1),
                ScrollDirection::Left => self.scroll_left(1),
                ScrollDirection::Right => self.scroll_right(1),
            })
            .action(|action, propagate| match action {
                // Accept regular OR scroll directional actions
                Action::Up | Action::ScrollUp => self.scroll_up(1),
                Action::Down | Action::ScrollDown => self.scroll_down(1),
                // Don't eat Left/Right arrows because those control tabs
                Action::ScrollLeft => self.scroll_left(1),
                Action::ScrollRight => self.scroll_right(1),
                Action::PageUp => {
                    self.scroll_up(self.window_size.get().height.into());
                }
                Action::PageDown => {
                    self.scroll_down(self.window_size.get().height.into());
                }
                Action::Home => self.scroll_to(0),
                // Clamping will limit this at the last line
                Action::End => self.scroll_to(usize::MAX),
                _ => propagate.set(),
            })
    }
}

/// `T` has to be convertible to text to be drawn
impl Draw<TextWindowProps> for TextWindow {
    fn draw(
        &self,
        canvas: &mut Canvas,
        props: TextWindowProps,
        metadata: DrawMetadata,
    ) {
        let gutter = Gutter {
            text_size: self.text_size,
            offset: self.offset.get(),
        };

        let [gutter_area, _, text_area] = Layout::horizontal([
            Constraint::Length(gutter.width()),
            Constraint::Length(1), // Spacer
            Constraint::Min(0),
        ])
        .areas(metadata.area());

        // Store window size for calculations in the update code
        let window_size = text_area.as_size();
        self.window_size.set(window_size);
        self.clamp_scroll(); // Revalidate scroll state if window size changes

        // Draw gutter and text
        canvas.render_widget(gutter, gutter_area);
        self.render_text(canvas.buffer_mut(), text_area);

        // Scrollbars
        let has_horizontal_scroll =
            self.text_size.width > window_size.width.into();
        let has_vertical_scroll =
            self.text_size.height > window_size.height.into();
        let offset = self.offset.get();
        if has_vertical_scroll {
            canvas.render_widget(
                Scrollbar {
                    content_length: self.text_size.height,
                    offset: offset.y,
                    margin: props.margins.right,
                    ..Default::default()
                },
                text_area,
            );
        }
        if has_horizontal_scroll {
            canvas.render_widget(
                Scrollbar {
                    content_length: self.text_size.width,
                    offset: offset.x,
                    orientation: ScrollbarOrientation::HorizontalBottom,
                    margin: props.margins.bottom,
                    invert: false,
                },
                text_area,
            );
        }
    }
}

/// Draw props for [TextWindow]
#[derive(Clone, Debug, Default)]
pub struct TextWindowProps {
    /// See [ScrollbarMargins]
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

/// Widget to draw line numbers in the left gutter
struct Gutter {
    text_size: TextSize,
    offset: Offset,
}

impl Gutter {
    fn width(&self) -> u16 {
        // Width is the number of digits in the biggest number
        (self.text_size.height as f32).log10().floor() as u16 + 1
    }
}

impl Widget for Gutter {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let styles = ViewContext::styles();
        // Draw line numbers in the gutter
        let first_line = self.offset.y + 1;
        let last_line = cmp::min(
            self.text_size.height,
            first_line + usize::from(area.height),
        );
        let text = (first_line..=last_line)
            .map(|n| Line::from(n.to_string()))
            .collect::<Text>()
            .alignment(Alignment::Right)
            .style(styles.text_window.gutter);
        text.render(area, buf);
    }
}

/// Lines/columns in a text body
#[derive(Copy, Clone, Debug)]
struct TextSize {
    /// Number of characters in the longest line of the text
    width: usize,
    /// Number of lines in the text
    height: usize,
}

impl TextSize {
    /// Compute the size of the given text. This is an **expensive** operation,
    /// because it has to count the number of characters in each line
    fn new(text: &Text) -> Self {
        let lines = &text.lines;
        // This counts _graphemes_, not bytes, so it's O(byte len)
        let mut width = 0;
        for line in lines {
            // For large files, counting graphemes could be expensive on every
            // line. All characters are >= 1 byte, so if a line has fewer bytes
            // than the current max width, it can't possibly be bigger
            let byte_len: usize =
                line.spans.iter().map(|span| span.content.len()).sum();
            if byte_len > width {
                width = width.max(line.width());
            }
        }
        let width = lines.iter().map(Line::width).max().unwrap_or_default();
        // Assume no line wrapping when calculating line count
        let height = lines.len();
        Self { width, height }
    }
}

/// Horizontal/vertical scroll offset
#[derive(Copy, Clone, Debug, Default, PartialEq)]
struct Offset {
    x: usize,
    y: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_util::{TestTerminal, terminal},
        view::test_util::{TestComponent, TestHarness, harness},
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
            Text::from("łïne 1\nłïne 2 is longer\nłïne 3\nłïne 4\nłïne 5");
        let props = TextWindowProps {
            // Don't overflow the frame
            margins: ScrollbarMargins {
                right: 0,
                bottom: 0,
            },
        };
        let mut component =
            TestComponent::builder(&harness, &terminal, TextWindow::new(text))
                .with_props(props.clone())
                .build();
        terminal.assert_buffer_lines([
            vec![line_num(1), " łïne 1 ▲".into()],
            vec![line_num(2), " łïne 2 █".into()],
            vec![line_num(3), " łïne 3 █".into()],
            vec![line_num(4), " ◀■■■═══▶".into()],
        ]);

        // Scroll down
        component
            .int_props(|| props.clone())
            .send_key(KeyCode::Down)
            .assert()
            .empty();
        terminal.assert_buffer_lines([
            vec![line_num(2), " łïne 2 ▲".into()],
            vec![line_num(3), " łïne 3 █".into()],
            vec![line_num(4), " łïne 4 █".into()],
            vec![line_num(5), " ◀■■■═══▶".into()],
        ]);

        // Scroll back up
        component
            .int_props(|| props.clone())
            // Second does nothing
            .send_keys([KeyCode::Up, KeyCode::Up])
            .assert()
            .empty();
        terminal.assert_buffer_lines([
            vec![line_num(1), " łïne 1 ▲".into()],
            vec![line_num(2), " łïne 2 █".into()],
            vec![line_num(3), " łïne 3 █".into()],
            vec![line_num(4), " ◀■■■═══▶".into()],
        ]);

        // Scroll right
        component
            .int_props(|| props.clone())
            .send_key_modifiers(KeyCode::Right, KeyModifiers::SHIFT)
            .send_key_modifiers(KeyCode::Right, KeyModifiers::SHIFT)
            .send_key_modifiers(KeyCode::Right, KeyModifiers::SHIFT)
            .assert()
            .empty();
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
            .assert()
            .empty();
        terminal.assert_buffer_lines([
            vec![line_num(1), " łïne 1 ▲".into()],
            vec![line_num(2), " łïne 2 █".into()],
            vec![line_num(3), " łïne 3 █".into()],
            vec![line_num(4), " ◀■■■═══▶".into()],
        ]);
    }

    #[rstest]
    fn test_unicode(
        #[with(35, 3)] terminal: TestTerminal,
        harness: TestHarness,
    ) {
        let text = Text::from("intro\n💚💙💜 this is a longer line\noutro");
        TestComponent::builder(&harness, &terminal, TextWindow::new(text))
            .with_props(TextWindowProps {
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
        let text = Text::raw("💚💙💜💚💙💜");
        TestComponent::builder(&harness, &terminal, TextWindow::new(text))
            .with_props(TextWindowProps {
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

    /// Growing the window reduces the maximum scroll. Scroll state should
    /// automatically be clamped to match
    #[rstest]
    fn test_grow_window(terminal: TestTerminal, harness: TestHarness) {
        let text =
            Text::from_iter(["1 this is a long line", "2", "3", "4", "5"]);
        let props = TextWindowProps {
            // Don't overflow the frame
            margins: ScrollbarMargins {
                right: 0,
                bottom: 0,
            },
        };
        let mut component =
            TestComponent::builder(&harness, &terminal, TextWindow::new(text))
                .with_props(props.clone())
                .build();

        component.set_area(Rect::new(0, 0, 10, 3));
        component
            .int_props(|| props.clone())
            .drain_draw()
            .assert()
            .empty();

        // Scroll out a bit
        component.scroll_down(2);
        component.scroll_right(10);
        assert_eq!(component.offset.get(), Offset { x: 10, y: 2 });

        component.set_area(Rect::new(0, 0, 15, 4));
        component
            .int_props(|| props.clone())
            .drain_draw()
            .assert()
            .empty();

        assert_eq!(component.offset.get(), Offset { x: 8, y: 1 });
    }

    /// Style some text as gutter line numbers
    fn line_num(n: u16) -> Span<'static> {
        let s = if n > 0 { n.to_string() } else { " ".into() };
        Span::styled(s, ViewContext::styles().text_window.gutter)
    }
}
