use crate::view::{
    Component, ViewContext,
    common::text_box::{TextBox, TextBoxEvent, TextBoxProps},
    component::{Canvas, Child, ComponentId, Draw, DrawMetadata, ToChild},
    context::UpdateContext,
    event::{Emitter, Event, EventMatch, ToEmitter},
};
use persisted::PersistedContainer;
use slumber_config::Action;
use std::mem;

/// A text box for executing shell commands. Includes command history navigation
#[derive(Debug)]
pub struct CommandTextBox {
    id: ComponentId,
    emitter: Emitter<CommandTextBoxEvent>,
    text_box: TextBox,
    /// Access previous commands with up/down arrow keys
    scrollback: Scrollback,
}

impl CommandTextBox {
    pub fn new(text_box: TextBox) -> Self {
        Self {
            id: ComponentId::default(),
            emitter: Emitter::default(),
            text_box: text_box.subscribe([
                TextBoxEvent::Cancel,
                TextBoxEvent::Change,
                TextBoxEvent::Focus,
                TextBoxEvent::Submit,
            ]),
            scrollback: Scrollback::Inactive,
        }
    }

    /// Get the visible text
    pub fn text(&self) -> &str {
        self.text_box.text()
    }

    /// Set the visible text
    pub fn set_text(&mut self, text: String) {
        self.text_box.set_text(text);
    }

    /// Clear the visible text and return what was cleared
    pub fn clear(&mut self) -> String {
        self.text_box.clear()
    }

    /// Go back one command in history
    fn scrollback_back(&mut self) {
        // If this is the first scrollback step, the helper will need to store
        // the current text so it can restore it when exiting scrollback
        if let Some(command) = self.scrollback.back(self.text_box.text()) {
            self.set_text(command);
        }
    }

    /// Go forward one command in history
    fn scrollback_forward(&mut self) {
        if let Some(command) = self.scrollback.forward() {
            self.set_text(command);
        }
    }

    /// Exit scrollback mode. Should be called after cancel OR submit to revert
    /// to the front of the scrollback queue
    fn reset_scrollback(&mut self) {
        self.scrollback = Scrollback::Inactive;
    }
}

impl Component for CommandTextBox {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn update(&mut self, _: &mut UpdateContext, event: Event) -> EventMatch {
        event
            .m()
            .action(|action, propagate| match action {
                Action::Up => self.scrollback_back(),
                Action::Down => self.scrollback_forward(),
                _ => propagate.set(),
            })
            .emitted(self.text_box.to_emitter(), |event| match event {
                TextBoxEvent::Focus => {
                    self.emitter.emit(CommandTextBoxEvent::Focus);
                }
                TextBoxEvent::Change => {}
                TextBoxEvent::Cancel => {
                    self.reset_scrollback();
                    self.emitter.emit(CommandTextBoxEvent::Cancel);
                }
                TextBoxEvent::Submit => {
                    // If we've submitted from scrollback, reset scrollback so
                    // we go to the front of the queue again
                    self.reset_scrollback();
                    self.emitter.emit(CommandTextBoxEvent::Submit);
                }
            })
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        vec![self.text_box.to_child_mut()]
    }
}

impl Draw<TextBoxProps> for CommandTextBox {
    fn draw(
        &self,
        canvas: &mut Canvas,
        props: TextBoxProps,
        metadata: DrawMetadata,
    ) {
        canvas.draw(
            &self.text_box,
            props,
            metadata.area(),
            metadata.has_focus(),
        );
    }
}

impl ToEmitter<CommandTextBoxEvent> for CommandTextBox {
    fn to_emitter(&self) -> Emitter<CommandTextBoxEvent> {
        self.emitter
    }
}

impl PersistedContainer for CommandTextBox {
    type Value = String;

    fn get_to_persist(&self) -> Self::Value {
        self.text_box.get_to_persist()
    }

    fn restore_persisted(&mut self, value: Self::Value) {
        self.text_box.restore_persisted(value);
    }
}

/// Emitted event for [CommandTextBox]
#[derive(Debug, PartialEq)]
pub enum CommandTextBoxEvent {
    Focus,
    Cancel,
    Submit,
}

/// State for history scrollback mode. User can navigation past commands with
/// up/down arrow keys.
#[derive(Debug, PartialEq)]
enum Scrollback {
    /// We're not in scrollback mode
    Inactive,
    /// We're in scrollback mode, showing a past command
    Active {
        /// Whatever text was present in the box when we entered scrollback
        /// mode. We keep this so we can restore it when exiting scrollback.
        /// This allows the enter/exit operation to be symmetrical.
        original: String,
        /// How many commands back have we gone? 0 is the most recent command,
        /// and increases go further back in history
        offset: usize,
    },
}

impl Scrollback {
    /// Go back one command in history. If scrollback mode isn't active, enter
    /// it
    fn back(&mut self, original: &str) -> Option<String> {
        match self {
            Self::Inactive => {
                // Exclude the original command from the history search to
                // prevent duplicates
                let command = Self::get(0, original);
                if command.is_some() {
                    // If this offset is valid, activate scrollback mode
                    *self = Self::Active {
                        original: original.to_owned(),
                        offset: 0,
                    };
                }
                command
            }
            Self::Active { original, offset } => {
                let new_offset = *offset + 1;
                let command = Self::get(new_offset, original);
                if command.is_some() {
                    // If this offset is valid, store it
                    *offset = new_offset;
                }
                command
            }
        }
    }

    /// Go forward one command in history. If we're already at the most recent
    /// command, exit scrollback and restore the original command.
    fn forward(&mut self) -> Option<String> {
        match self {
            // We're not scrolled back, so we can't go forward
            Self::Inactive => None,
            // We're already on the most recent command, so exit scrollback and
            // return the text from when scrollback was entered
            Self::Active { original, offset } if *offset == 0 => {
                let command = mem::take(original);
                *self = Self::Inactive;
                Some(command)
            }
            // Remain in scrollback, but go forward one
            Self::Active { original, offset } => {
                *offset -= 1;
                // We expect this to always return Some, since we already had
                // something further back selected
                Self::get(*offset, original)
            }
        }
    }

    /// Get the historical command at the given offset. The original command
    /// (whatever was in the text box when the user entered scrollback) is
    /// always excluded from historical results.
    fn get(offset: usize, exclude: &str) -> Option<String> {
        ViewContext::with_database(|db| db.get_command(offset, exclude))
            .unwrap_or(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_util::{TestHarness, TestTerminal, harness, terminal},
        view::test_util::TestComponent,
    };
    use rstest::{fixture, rstest};
    use terminput::KeyCode;

    impl Scrollback {
        fn active(original: &str, offset: usize) -> Self {
            Self::Active {
                original: original.to_owned(),
                offset,
            }
        }
    }

    /// Initialize the DB with some command history
    #[fixture]
    fn scrollback_db(_harness: TestHarness) {
        ViewContext::with_database(|db| {
            // These are served in reverse order: 0 => three, 1 => two, 2 => one
            db.insert_command("one").unwrap();
            db.insert_command("two").unwrap();
            db.insert_command("three").unwrap();
        });
    }

    /// Scroll back/forth through command history
    #[rstest]
    fn test_component_scrollback(harness: TestHarness, terminal: TestTerminal) {
        ViewContext::with_database(|db| {
            // These are served in reverse order: 0 => three, 1 => two, 2 => one
            db.insert_command("one").unwrap();
            db.insert_command("two").unwrap();
            db.insert_command("three").unwrap();
        });

        let mut component = TestComponent::new(
            &harness,
            &terminal,
            CommandTextBox::new(TextBox::default()),
        );

        // Scroll back
        assert_eq!(component.text(), "");
        component.int().send_key(KeyCode::Up).assert_empty();
        assert_eq!(component.text(), "three");

        // Scroll forward
        component
            .int()
            .send_keys([KeyCode::Up, KeyCode::Up, KeyCode::Down])
            .assert_empty();
        assert_eq!(component.text(), "two");

        // Submit
        component
            .int()
            .send_key(KeyCode::Enter)
            .assert_emitted([CommandTextBoxEvent::Submit]);
        assert_eq!(component.text(), "two");

        // Submission resets scrollback state, so now when we go back from two
        // we get three instead of one
        component.int().send_key(KeyCode::Up).assert_empty();
        assert_eq!(component.text(), "three");
        component.int().send_key(KeyCode::Up).assert_empty();
        assert_eq!(component.text(), "one");
    }

    /// Various scenarios scrolling back in history
    #[rstest]
    // While scrollback is inactive, enter scrollback
    #[case::enter(
        Scrollback::Inactive,
        Some("three"),
        Scrollback::active("orig", 0)
    )]
    // While in scrollback, go back one command
    #[case::back(
        Scrollback::active("orig", 0),
        Some("two"),
        Scrollback::active("orig", 1)
    )]
    // While in scrollback, attempt to go back but there's nothing left
    #[case::back_noop(
        Scrollback::active("orig", 2),
        None,
        Scrollback::active("orig", 2)
    )]
    fn test_scrollback_back(
        _scrollback_db: (),
        #[case] mut initial: Scrollback,
        #[case] expected_output: Option<&str>,
        #[case] expected_scrollback: Scrollback,
    ) {
        assert_eq!(initial.back("orig").as_deref(), expected_output);
        assert_eq!(initial, expected_scrollback);
    }

    /// Various scenarios scrolling forward in history
    #[rstest]
    // While in scrollback, go forward one command
    #[case::forward(
        Scrollback::active("orig", 1),
        Some("three"),
        Scrollback::active("orig", 0)
    )]
    // While *not* in scrollback, attempt to go forward but we can't
    #[case::forward_noop(Scrollback::Inactive, None, Scrollback::Inactive)]
    // While in scrollback at the most recent item, go forward to exit
    #[case::exit(
        Scrollback::active("orig", 0),
        Some("orig"),
        Scrollback::Inactive
    )]
    fn test_scrollback_forward(
        _scrollback_db: (),
        #[case] mut initial: Scrollback,
        #[case] expected_output: Option<&str>,
        #[case] expected_scrollback: Scrollback,
    ) {
        assert_eq!(initial.forward().as_deref(), expected_output);
        assert_eq!(initial, expected_scrollback);
    }

    /// Original command should be excluded from all scrollback suggestions
    #[rstest]
    fn test_scrollback_exclude_original(_scrollback_db: ()) {
        let mut scrollback = Scrollback::Inactive;
        // "three" is excluded entirely from the history list, so the offset is
        // still 0 but it points to "two"
        assert_eq!(scrollback.back("three"), Some("two".into()));
        assert_eq!(scrollback, Scrollback::active("three", 0));
    }
}
