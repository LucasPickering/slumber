use crate::{
    context::TuiContext,
    util,
    view::{
        Generate, UpdateContext, ViewContext,
        component::{
            Canvas, Component, ComponentId, Draw, DrawMetadata,
            help::HelpFooter,
        },
        event::{Emitter, Event, EventMatch},
        state::Notification,
    },
};
use ratatui::{
    layout::{Constraint, Layout},
    text::Span,
};
use slumber_config::Action;
use slumber_core::database::CollectionDatabase;
use slumber_util::ResultTraced;
use tokio::time;
use uuid::Uuid;

/// Component at the bottom
#[derive(Debug, Default)]
pub struct Footer {
    id: ComponentId,
    notification: Option<Notification>,
    clear_emitter: Emitter<ClearNotification>,
}

impl Footer {
    /// Display an informational message to the user
    pub fn notify(&mut self, message: String) {
        let notification = Notification::new(message.to_string());
        let id = notification.id;
        self.notification = Some(notification);
        let emitter = self.clear_emitter;
        // Spawn a task to clear the notification
        // Hack alert! We skip this in tests because spawning a local task adds
        // accidental complexity. Since this task is a fixed length, it slows
        // tests down a lot.
        if !cfg!(test) {
            util::spawn(async move {
                time::sleep(Notification::DURATION).await;
                emitter.emit(ClearNotification(id));
            });
        }
    }
}

impl Component for Footer {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn update(&mut self, _: &mut UpdateContext, event: Event) -> EventMatch {
        event
            .m()
            .emitted(self.clear_emitter, |ClearNotification(id)| {
                // Clear the notification only if the clear message matches what
                // we have. This prevents early clears when multiple
                // notifcations are send in quick succession
                if self.notification.as_ref().is_some_and(|n| n.id == id) {
                    self.notification = None;
                }
            })
    }
}

impl Draw for Footer {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        // If a notification is present, it gets the entire footer.
        // Notifications are auto-cleared so it's ok to hide other stuff
        // temporarily
        if let Some(notification) = &self.notification {
            canvas
                .render_widget(notification.message.as_str(), metadata.area());
            return;
        }

        // No notification - show help dialog and current collection path
        let input_engine = &TuiContext::get().input_engine;
        let styles = &TuiContext::get().styles;

        let collection_name =
            ViewContext::with_database(CollectionDatabase::metadata)
                .map(|metadata| metadata.display_name())
                .traced()
                .unwrap_or_default();
        let collection_name_text = Span::styled(
            input_engine.add_hint(collection_name, Action::SelectCollection),
            styles.text.highlight,
        );

        let help = HelpFooter.generate();
        let [collection_area, help_area] = Layout::horizontal([
            Constraint::Length(collection_name_text.content.len() as u16),
            Constraint::Min(help.width() as u16),
        ])
        .areas(metadata.area());

        canvas.render_widget(collection_name_text, collection_area);
        canvas.render_widget(help.into_right_aligned_line(), help_area);
    }
}

/// Emitted event to clear a particular notification
#[derive(Debug)]
struct ClearNotification(Uuid);
