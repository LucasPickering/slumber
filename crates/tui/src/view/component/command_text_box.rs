use crate::view::{
    Component, ViewContext,
    common::{
        select::{Select, SelectEventKind, SelectListProps},
        text_box::{TextBox, TextBoxEvent, TextBoxProps},
    },
    component::{Canvas, Child, ComponentId, Draw, DrawMetadata, ToChild},
    context::UpdateContext,
    event::{Emitter, Event, EventMatch, ToEmitter},
};
use ratatui::{
    layout::{Offset, Rect},
    widgets::{Clear, ListDirection},
};
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
    /// Results from ctrl-r search. `Some` only when the search is visible and
    /// navigable
    search: Option<Select<String>>,
}

impl CommandTextBox {
    pub fn new(text_box: TextBox) -> Self {
        Self {
            id: ComponentId::default(),
            emitter: Emitter::default(),
            text_box: text_box.subscribe([
                TextBoxEvent::Cancel,
                TextBoxEvent::Change,
                TextBoxEvent::Submit,
            ]),
            scrollback: Scrollback::Inactive,
            search: None,
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

    /// Search for commands matching the current text. If there are any results,
    /// open them in a list
    fn update_search(&mut self) {
        let query = self.text();
        let commands = ViewContext::with_database(|db| db.get_commands(query))
            .unwrap_or_default(); // Error should be logged by the DB
        if commands.is_empty() {
            self.search = None;
        } else {
            // Load ALL the results into a select. draw() is responsible for
            // limiting what's visible at a time. The DB caps the history
            // length so this is bounded.
            self.search = Some(
                Select::builder(commands)
                    // Most recent command is closest to the text box
                    .direction(ListDirection::BottomToTop)
                    .subscribe([SelectEventKind::Submit])
                    .build(),
            );
        }
    }

    /// Cancel the search without taking its selection
    fn close_search(&mut self) {
        self.search = None;
    }

    /// Close the search and submit the currently selected item as a command
    fn submit_search(&mut self) {
        // *should* always be true, as this is only called when the select is
        // defined and has something selected
        if let Some(command) =
            self.search.take().and_then(Select::into_selected)
        {
            self.set_text(command);
            self.emitter.emit(CommandTextBoxEvent::Submit);
        }
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
                Action::SearchHistory => self.update_search(),
                _ => propagate.set(),
            })
            .emitted_opt(
                self.search.as_ref().map(ToEmitter::to_emitter),
                |event| match event.kind {
                    SelectEventKind::Submit => self.submit_search(),
                    SelectEventKind::Select | SelectEventKind::Toggle => {}
                },
            )
            .emitted(self.text_box.to_emitter(), |event| match event {
                TextBoxEvent::Change => {
                    // If searching, update the search results
                    if self.search.is_some() {
                        self.update_search();
                    }
                }
                TextBoxEvent::Cancel => {
                    self.reset_scrollback();
                    self.close_search();
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
        vec![self.search.to_child_mut(), self.text_box.to_child_mut()]
    }
}

impl Draw<TextBoxProps> for CommandTextBox {
    fn draw(
        &self,
        canvas: &mut Canvas,
        props: TextBoxProps,
        metadata: DrawMetadata,
    ) {
        let area = metadata.area();
        canvas.draw(&self.text_box, props, area, metadata.has_focus());

        if let Some(search) = &self.search {
            let height = search.len().min(10) as u16;
            // We're intentionally blowing out our area here to render on top
            // of the pane above
            let search_area = Rect {
                height,
                // Shift up from the text box to account for the height
                ..area.offset(Offset {
                    x: 0,
                    y: -i32::from(height),
                })
            }
            .intersection(canvas.area()); // Don't go outside terminal
            canvas.render_widget(Clear, search_area); // Clear styling
            canvas.draw(
                search,
                SelectListProps {
                    scrollbar_margin: 0,
                },
                search_area,
                true,
            );
        }
    }
}

impl ToEmitter<CommandTextBoxEvent> for CommandTextBox {
    fn to_emitter(&self) -> Emitter<CommandTextBoxEvent> {
        self.emitter
    }
}

/// Emitted event for [CommandTextBox]
#[derive(Debug, PartialEq)]
pub enum CommandTextBoxEvent {
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
        offset: u32,
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
    fn get(offset: u32, exclude: &str) -> Option<String> {
        ViewContext::with_database(|db| db.get_command(offset, exclude))
            .unwrap_or(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_util::{TestTerminal, terminal},
        view::{
            context::ViewContext,
            test_util::{TestComponent, TestHarness, harness},
        },
    };
    use ratatui::{style::Styled, text::Line};
    use rstest::{fixture, rstest};
    use terminput::{KeyCode, KeyModifiers};

    impl Scrollback {
        fn active(original: &str, offset: u32) -> Self {
            Self::Active {
                original: original.to_owned(),
                offset,
            }
        }
    }

    /// Scroll back/forth through command history
    #[rstest]
    fn test_component_scrollback(
        harness: TestHarness,
        terminal: TestTerminal,
        _history_db: (),
    ) {
        let mut component = TestComponent::new(
            &harness,
            &terminal,
            CommandTextBox::new(TextBox::default()),
        );

        // Scroll back
        assert_eq!(component.text(), "");
        component.int().send_key(KeyCode::Up).assert().empty();
        assert_eq!(component.text(), "three");

        // Scroll forward
        component
            .int()
            .send_keys([KeyCode::Up, KeyCode::Up, KeyCode::Down])
            .assert()
            .empty();
        assert_eq!(component.text(), "two");

        // Submit
        component
            .int()
            .send_key(KeyCode::Enter)
            .assert()
            .emitted([CommandTextBoxEvent::Submit]);
        assert_eq!(component.text(), "two");

        // Submission resets scrollback state, so now when we go back from two
        // we get three instead of one
        component.int().send_key(KeyCode::Up).assert().empty();
        assert_eq!(component.text(), "three");
        component.int().send_key(KeyCode::Up).assert().empty();
        assert_eq!(component.text(), "one");
    }

    /// Search history with ctrl+r
    #[rstest]
    fn test_component_search(
        harness: TestHarness,
        #[with(6, 3)] terminal: TestTerminal,
        _history_db: (),
    ) {
        let styles = ViewContext::styles();
        let mut component = TestComponent::new(
            &harness,
            &terminal,
            CommandTextBox::new(TextBox::default()),
        );
        // The search box blows up out of the given area, so use the bottom
        // line for the text box
        component.set_area(bottom_row_area(&terminal));

        // Initial text should be used for the query
        component
            .int()
            .send_text("t")
            .send_key_modifiers(KeyCode::Char('r'), KeyModifiers::CTRL)
            .assert()
            .empty();
        assert_eq!(component.text(), "t");
        assert_eq!(get_search_items(&component).unwrap(), &["three", "two"]);
        terminal.assert_buffer_lines([
            // Most recent last!!
            Line::from("two   "),
            Line::from("three ".set_style(styles.list.highlight)),
            // Text box line needs specific styling
            Line::from_iter([
                "t".set_style(styles.text_box.text),
                " ".set_style(styles.text_box.cursor),
                "    ".set_style(styles.text_box.text),
            ]),
        ]);

        // Modifying while in search mode should update what's visible
        component.int().send_text("h").assert().empty();
        assert_eq!(component.text(), "th");
        assert_eq!(get_search_items(&component).unwrap(), &["three"]);

        // Enter closes the search AND submits
        component
            .int()
            .send_key(KeyCode::Enter)
            .assert()
            .emitted([CommandTextBoxEvent::Submit]);
        assert_eq!(component.text(), "three");
    }

    /// Search history with ctrl+r. Escape exits the query *without* taking the
    /// selected item
    #[rstest]
    fn test_component_search_cancel(
        harness: TestHarness,
        #[with(6, 3)] terminal: TestTerminal,
        _history_db: (),
    ) {
        let mut component = TestComponent::new(
            &harness,
            &terminal,
            CommandTextBox::new(TextBox::default()),
        );
        component.set_area(bottom_row_area(&terminal));

        component
            .int()
            .send_text("t")
            .send_key_modifiers(KeyCode::Char('r'), KeyModifiers::CTRL)
            .assert()
            .empty();
        assert_eq!(get_search_items(&component).unwrap(), &["three", "two"]);

        // Escape exits without modifying the text. This exits both the search
        // list *and* the text box.
        component
            .int()
            .send_key(KeyCode::Esc)
            .assert()
            .emitted([CommandTextBoxEvent::Cancel]);
        assert_eq!(component.text(), "t");
    }

    /// Search history with ctrl+r. When there are no matches, we do *not*
    /// enter search mode
    #[rstest]
    fn test_component_search_no_match(
        harness: TestHarness,
        #[with(6, 3)] terminal: TestTerminal,
        _history_db: (),
    ) {
        let mut component = TestComponent::new(
            &harness,
            &terminal,
            CommandTextBox::new(TextBox::default()),
        );
        component.set_area(bottom_row_area(&terminal));

        // Initial text should be used for the query
        component
            .int()
            .send_text("teefs")
            .send_key_modifiers(KeyCode::Char('r'), KeyModifiers::CTRL)
            .assert()
            .empty();
        assert_eq!(component.text(), "teefs");
        assert_eq!(get_search_items(&component), None);
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
        _harness: TestHarness, // Needed for ViewContext
        _history_db: (),
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
        _harness: TestHarness, // Needed for ViewContext
        _history_db: (),
        #[case] mut initial: Scrollback,
        #[case] expected_output: Option<&str>,
        #[case] expected_scrollback: Scrollback,
    ) {
        assert_eq!(initial.forward().as_deref(), expected_output);
        assert_eq!(initial, expected_scrollback);
    }

    /// Original command should be excluded from all scrollback suggestions
    #[rstest]
    fn test_scrollback_exclude_original(
        _harness: TestHarness, // Needed for ViewContext,
        _history_db: (),
    ) {
        let mut scrollback = Scrollback::Inactive;
        // "three" is excluded entirely from the history list, so the offset is
        // still 0 but it points to "two"
        assert_eq!(scrollback.back("three"), Some("two".into()));
        assert_eq!(scrollback, Scrollback::active("three", 0));
    }

    /// Initialize the DB with some command history. This needs to be pulled in
    /// *after* the `harness` fixture, because that one initializes
    /// `ViewContext`. We can't pull the fixture in here because then it gets
    /// initialized twice.
    #[fixture]
    fn history_db() {
        ViewContext::with_database(|db| {
            // These are served in reverse order: 0 => three, 1 => two, 2 => one
            db.insert_command("one").unwrap();
            db.insert_command("two").unwrap();
            db.insert_command("three").unwrap();
        });
    }

    /// Helper to get the visible search results
    fn get_search_items(component: &CommandTextBox) -> Option<Vec<&str>> {
        component.search.as_ref().map(|select| {
            select.items().map(String::as_str).collect::<Vec<_>>()
        })
    }

    /// Get the area of the bottom row of the terminal. This is where we render
    /// the text box to, so the history search can appear above it
    fn bottom_row_area(terminal: &TestTerminal) -> Rect {
        terminal.area().rows().next_back().unwrap()
    }
}
