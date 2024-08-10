use crate::view::draw::{Draw, DrawMetadata};
use ratatui::Frame;
use std::{cell::Cell, time::Instant};

/// Show developer information, including framerate
#[derive(Debug)]
pub struct DebugMonitor {
    last_frame: Cell<Instant>,
}

impl Default for DebugMonitor {
    fn default() -> Self {
        Self {
            last_frame: Instant::now().into(),
        }
    }
}

impl Draw for DebugMonitor {
    fn draw(&self, frame: &mut Frame, _: (), metadata: DrawMetadata) {
        let now = Instant::now();
        let duration = now - self.last_frame.replace(now);
        let fps = 1.0 / duration.as_secs_f32();
        frame.render_widget(format!("FPS: {fps:.2}"), metadata.area());
    }
}
