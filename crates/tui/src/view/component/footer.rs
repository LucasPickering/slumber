use crate::view::{
    UpdateContext, ViewContext,
    component::{
        Canvas, Child, Component, ComponentId, Draw, DrawMetadata, ToChild,
        collection_select::CollectionSelect,
    },
    event::{Emitter, Event, EventMatch},
    state::Notification,
};
use itertools::Itertools;
use ratatui::{
    layout::{Constraint, Layout},
    text::Span,
};
use slumber_config::Action;
use tokio::time;
use uuid::Uuid;

/// Component at the bottom
#[derive(Debug, Default)]
pub struct Footer {
    id: ComponentId,
    /// Display current collection with a list that can open to switch
    /// collections
    collection_select: CollectionSelect,
    notification: Option<Notification>,
    clear_emitter: Emitter<ClearNotification>,
}

impl Footer {
    /// Open the collection select menu
    pub fn open_collection_select(&mut self) {
        self.collection_select.open();
    }

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
            ViewContext::spawn(async move {
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
        vec![self.collection_select.to_child()]
    }
}

impl Draw for Footer {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        if let Some(notification) = &self.notification {
            // If a notification is present, it gets the entire footer.
            // Notifications are auto-cleared so it's ok to hide other stuff
            // temporarily
            canvas
                .render_widget(notification.message.as_str(), metadata.area());
        } else {
            // No notification - show collection selector and minimal help
            let [collection_area, help_area] = Layout::horizontal([
                Constraint::Length(self.collection_select.text().len() as u16),
                Constraint::Min(0),
            ])
            .areas(metadata.area());

            canvas.draw(
                &self.collection_select,
                (),
                collection_area,
                self.collection_select.is_open(),
            );

            // Help
            let actions = [Action::OpenActions, Action::Help, Action::Quit];
            let text = actions
                .into_iter()
                .map(|action| {
                    let binding = ViewContext::binding_display(action);
                    format!("{binding} {action}")
                })
                .join(" / ");

            let span = Span::styled(text, ViewContext::styles().text.highlight)
                .into_right_aligned_line();
            canvas.render_widget(span, help_area);
        }
    }
}

/// Emitted event to clear a particular notification
#[derive(Debug)]
struct ClearNotification(Uuid);
