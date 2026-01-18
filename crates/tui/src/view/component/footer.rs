use crate::{
    context::TuiContext,
    util,
    view::{
        UpdateContext,
        component::{
            Canvas, Child, Component, ComponentId, Draw, DrawMetadata, ToChild,
            collection_select::CollectionSelect, help::Help,
        },
        event::{Emitter, Event, EventMatch},
        state::Notification,
    },
};
use ratatui::{
    layout::{Constraint, Layout},
    style::Stylize,
    widgets::{Block, Clear},
};
use tokio::time;
use uuid::Uuid;

/// Component at the bottom
#[derive(Debug, Default)]
pub struct Footer {
    id: ComponentId,
    /// Show minimal help info in the footer, and open up to a fullscreen help
    /// page
    help: Help,
    /// Display current collection with a list that can open to switch
    /// collections
    collection_select: CollectionSelect,
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

    fn children(&mut self) -> Vec<Child<'_>> {
        vec![
            self.help.to_child_mut(),
            self.collection_select.to_child_mut(),
        ]
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
        let [collection_area, help_area] = Layout::horizontal([
            Constraint::Length(self.collection_select.text().len() as u16),
            Constraint::Min(0),
        ])
        .areas(metadata.area());

        canvas.render_widget(
            Block::new().bg(TuiContext::get().styles.table.background_color),
            metadata.area(),
        );

        canvas.draw(&self.collection_select, (), collection_area, true);

        // Draw help last. If it's in fullscreen mode, it draws over everything
        // else
        canvas.draw(&self.help, (), help_area, true);
    }
}

/// Emitted event to clear a particular notification
#[derive(Debug)]
struct ClearNotification(Uuid);
