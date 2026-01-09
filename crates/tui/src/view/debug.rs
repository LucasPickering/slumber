use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Layout},
    style::{Color, Style},
    text::Text,
    widgets::{Paragraph, Widget},
};
use std::{cell::Cell, time::Instant};

/// Globally track debug/performance information. This implements
/// [tracing::Subscriber] to collect data.
#[derive(Debug)]
pub struct DebugMonitor {
    /// Track the start of the previous draw, so we can calculate frame rate
    last_draw_start: Cell<Instant>,
}

impl DebugMonitor {
    /// Draw the view using the given closure, then render computed metrics on
    /// top at the end.
    pub fn draw<T>(
        &self,
        buffer: &mut Buffer,
        draw_fn: impl FnOnce(&mut Buffer) -> T,
    ) -> T {
        // Track elapsed time for the draw function
        let start = Instant::now();
        let output = draw_fn(buffer);
        let duration = start.elapsed();
        let fps = 1.0 / (start - self.last_draw_start.get()).as_secs_f32();
        self.last_draw_start.set(start);

        // Draw in the bottom-right, on top of the help text
        let [_, area] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(1)])
                .areas(*buffer.area());
        let text = Text::from(format!(
            "FPS: {fps:.1} / Render: {duration}ms",
            duration = duration.as_millis()
        ))
        .style(Style::default().fg(Color::Black).bg(Color::Green));
        Widget::render(
            Paragraph::new(text).alignment(Alignment::Right),
            area,
            buffer,
        );
        output
    }
}

impl Default for DebugMonitor {
    fn default() -> Self {
        Self {
            last_draw_start: Instant::now().into(),
        }
    }
}
