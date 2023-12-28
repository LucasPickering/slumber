use crate::tui::{
    context::TuiContext,
    input::Action,
    message::Message,
    view::{
        common::{list::List, modal::Modal},
        draw::{Draw, Generate},
        event::{Event, EventHandler, Update, UpdateContext},
        state::select::{Fixed, SelectState},
        util::layout,
        Component, ModalPriority,
    },
};
use anyhow::anyhow;
use derive_more::Display;
use ratatui::{
    prelude::{Alignment, Constraint, Direction, Rect},
    text::{Line, Span, Text},
    widgets::{ListState, Paragraph},
    Frame,
};
use std::{cell::Cell, cmp, fmt::Debug};
use strum::{EnumCount, EnumIter};

/// A scrollable (but not editable) block of text. Text is not externally
/// mutable. If you need to update the text, store this in a `StateCell` and
/// reconstruct the entire component.
///
/// The generic parameter allows for any type that can be converted to ratatui's
/// `Text`, e.g. `String` or `TemplatePreview`.
#[derive(derive_more::Debug)]
pub struct TextWindow<T> {
    #[debug(skip)]
    text: T,
    offset_y: u16,
    text_height: Cell<u16>,
    window_height: Cell<u16>,
}

impl<T> TextWindow<T> {
    pub fn new(text: T) -> Self {
        Self {
            text,
            offset_y: 0,
            text_height: Cell::default(),
            window_height: Cell::default(),
        }
    }

    /// Get the final line that we can't scroll past. This will be the first
    /// line of the last page of text
    fn max_scroll_line(&self) -> u16 {
        self.text_height
            .get()
            .saturating_sub(self.window_height.get())
    }

    fn scroll_up(&mut self, lines: u16) {
        self.offset_y = self.offset_y.saturating_sub(lines);
    }

    fn scroll_down(&mut self, lines: u16) {
        self.offset_y = cmp::min(self.offset_y + lines, self.max_scroll_line());
    }

    /// Scroll to a specific line number. The target line will end up as close
    /// to the top of the page as possible
    fn scroll_to(&mut self, line: u16) {
        self.offset_y = cmp::min(line, self.max_scroll_line());
    }

    /// Copy all text in the window to the clipboard
    fn copy_text(&self, context: &mut UpdateContext)
    where
        T: ToString,
    {
        match cli_clipboard::set_contents(self.text.to_string()) {
            Ok(()) => {
                context.notify("Copied text to clipboard");
            }
            Err(error) => {
                // Returned error doesn't impl 'static so we can't
                // directly convert it to anyhow
                TuiContext::send_message(Message::Error {
                    error: anyhow!("Error copying text: {error}"),
                })
            }
        }
    }
}

/// ToString required for copy action
impl<T: Debug + ToString> EventHandler for TextWindow<T> {
    fn update(&mut self, context: &mut UpdateContext, event: Event) -> Update {
        match event {
            Event::Input {
                action: Some(action),
                ..
            } => match action {
                Action::Up | Action::ScrollUp => self.scroll_up(1),
                Action::Down | Action::ScrollDown => self.scroll_down(1),
                Action::PageUp => self.scroll_up(self.window_height.get()),
                Action::PageDown => self.scroll_down(self.window_height.get()),
                Action::Home => self.scroll_to(0),
                Action::End => self.scroll_to(u16::MAX),
                Action::OpenActions => context.open_modal(
                    TextWindowActionsModal::default(),
                    ModalPriority::Low,
                ),
                _ => return Update::Propagate(event),
            },
            Event::CopyText => self.copy_text(context),
            _ => return Update::Propagate(event),
        }
        Update::Consumed
    }
}

impl<'a, T> Draw for &'a TextWindow<T>
where
    &'a T: 'a + Generate<Output<'a> = Text<'a>>,
{
    fn draw(&self, frame: &mut Frame, _: (), area: Rect) {
        let theme = &TuiContext::get().theme;
        let text = self.text.generate();
        let text_height = text.lines.len() as u16;
        self.text_height.set(text_height);
        self.window_height.set(area.height);

        let [gutter_area, _, text_area] = layout(
            area,
            Direction::Horizontal,
            [
                // Size gutter based on width of max line number
                Constraint::Length(
                    (text_height as f32).log10().floor() as u16 + 1,
                ),
                Constraint::Length(1), // Spacer
                Constraint::Min(0),
            ],
        );

        // Draw line numbers in the gutter
        let first_line = self.offset_y + 1;
        let last_line = cmp::min(first_line + area.height, text_height);
        frame.render_widget(
            Paragraph::new(
                (first_line..=last_line)
                    .map(|n| n.to_string().into())
                    .collect::<Vec<Line>>(),
            )
            .alignment(Alignment::Right)
            .style(theme.line_number_style),
            gutter_area,
        );

        // Darw the text content
        frame.render_widget(
            Paragraph::new(self.text.generate()).scroll((self.offset_y, 0)),
            text_area,
        );
    }
}

/// Modal to trigger useful commands
#[derive(Debug)]
struct TextWindowActionsModal {
    actions: Component<SelectState<Fixed, TextWindowAction, ListState>>,
}

impl Default for TextWindowActionsModal {
    fn default() -> Self {
        fn on_submit(
            context: &mut UpdateContext,
            action: &mut TextWindowAction,
        ) {
            // Close the modal *first*, so the action event gets handled by our
            // parent rather than the modal. Jank but it works
            context.queue_event(Event::CloseModal);
            match action {
                TextWindowAction::Copy => context.queue_event(Event::CopyText),
            }
        }

        Self {
            actions: SelectState::fixed().on_submit(on_submit).into(),
        }
    }
}

impl Modal for TextWindowActionsModal {
    fn title(&self) -> &str {
        "Actions"
    }

    fn dimensions(&self) -> (Constraint, Constraint) {
        (
            Constraint::Length(30),
            Constraint::Length(TextWindowAction::COUNT as u16),
        )
    }
}

impl EventHandler for TextWindowActionsModal {
    fn children(&mut self) -> Vec<Component<&mut dyn EventHandler>> {
        vec![self.actions.as_child()]
    }
}

impl Draw for TextWindowActionsModal {
    fn draw(&self, frame: &mut Frame, _: (), area: Rect) {
        let list = List {
            block: None,
            list: &self.actions,
        };
        frame.render_stateful_widget(
            list.generate(),
            area,
            &mut self.actions.state_mut(),
        );
    }
}

/// Items in the actions popup menu
#[derive(
    Copy, Clone, Debug, Default, Display, EnumCount, EnumIter, PartialEq,
)]
enum TextWindowAction {
    #[default]
    Copy,
}

impl Generate for &TextWindowAction {
    type Output<'this> = Span<'this> where Self: 'this;

    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this,
    {
        self.to_string().into()
    }
}
