use crate::{
    context::TuiContext,
    util,
    view::{
        UpdateContext, ViewContext,
        component::help::HelpFooter,
        draw::{Draw, DrawMetadata, Generate},
        event::{Event, EventHandler, OptionEvent},
        state::Notification,
    },
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    text::Span,
};
use slumber_config::Action;
use slumber_util::ResultTraced;
use tokio::time;

/// Component at the bottom
#[derive(Debug, Default)]
pub struct Footer {
    notification: Option<Notification>,
}

impl EventHandler for Footer {
    fn update(&mut self, _: &mut UpdateContext, event: Event) -> Option<Event> {
        event.opt().any(|event| match event {
            Event::Notify(notification) => {
                let id = notification.id;
                self.notification = Some(notification);
                // Spawn a task to clear the notification
                util::spawn(async move {
                    time::sleep(Notification::DURATION).await;
                    ViewContext::push_event(Event::NotifyClear(id));
                });
                None
            }
            Event::NotifyClear(id) => {
                // Clear the notification only if the clear message matches what
                // we have. This prevents early clears when multiple
                // notifcations are send in quick succession
                if let Some(notification) = &self.notification
                    && notification.id == id
                {
                    self.notification = None;
                }
                None
            }

            _ => Some(event),
        })
    }
}

impl Draw for Footer {
    fn draw(&self, frame: &mut Frame, (): (), metadata: DrawMetadata) {
        // If a notification is present, it gets the entire footer.
        // Notifications are auto-cleared so it's ok to hide other stuff
        // temporarily
        if let Some(notification) = &self.notification {
            frame.render_widget(notification.message.as_str(), metadata.area());
            return;
        }

        // No notification - show help dialog and current collection path
        let input_engine = &TuiContext::get().input_engine;
        let styles = &TuiContext::get().styles;

        let collection_path =
            ViewContext::with_database(|db| Ok(db.metadata()?.path))
                .traced()
                .unwrap_or_default();
        let collection_path_text = Span::styled(
            input_engine
                .add_hint(collection_path.display(), Action::SelectCollection),
            styles.text.highlight,
        );

        let help = HelpFooter.generate();
        let [help_area, collection_area] = Layout::horizontal([
            Constraint::Min(help.width() as u16),
            Constraint::Length(collection_path_text.content.len() as u16),
        ])
        .areas(metadata.area());

        frame.render_widget(help, help_area);
        frame.render_widget(
            collection_path_text.into_right_aligned_line(),
            collection_area,
        );
    }
}
