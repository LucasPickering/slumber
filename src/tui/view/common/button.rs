//! Buttons and button accessories

use crate::tui::{
    context::TuiContext,
    input::Action,
    view::{
        draw::{Draw, DrawMetadata, Generate},
        event::{Event, EventHandler, Update},
        state::fixed_select::{FixedSelect, FixedSelectState},
        ViewContext,
    },
};
use ratatui::{
    layout::{Constraint, Flex, Layout},
    text::Span,
    Frame,
};

/// An piece of text that the user can "press" with the submit action. It should
/// only be interactable if it is focused, but that's up to the caller to
/// enforce.
pub struct Button<'a> {
    text: &'a str,
    has_focus: bool,
}

impl<'a> Generate for Button<'a> {
    type Output<'this> = Span<'this>
    where
        Self: 'this;

    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this,
    {
        let styles = &TuiContext::get().styles;
        Span {
            content: self.text.into(),
            style: if self.has_focus {
                styles.text.highlight
            } else {
                Default::default()
            },
        }
    }
}

/// A collection of buttons. User can cycle between buttons and hit enter to
/// activate one. When a button is activated, it will emit a dynamic event with
/// type `T`.
#[derive(Debug, Default)]
pub struct ButtonGroup<T: FixedSelect> {
    select: FixedSelectState<T>,
}

impl<T: FixedSelect> EventHandler for ButtonGroup<T> {
    fn update(&mut self, event: Event) -> Update {
        let Some(action) = event.action() else {
            return Update::Propagate(event);
        };
        match action {
            Action::Left => self.select.previous(),
            Action::Right => self.select.next(),
            Action::Submit => {
                // Propagate the selected item as a dynamic event
                ViewContext::push_event(Event::new_local(
                    self.select.selected(),
                ));
            }
            _ => return Update::Propagate(event),
        }
        Update::Consumed
    }

    // Do *not* treat the select state as a child, because the default select
    // action bindings aren't intuitive for this component
}

impl<T: FixedSelect> Draw for ButtonGroup<T> {
    fn draw(&self, frame: &mut Frame, _: (), metadata: DrawMetadata) {
        let items = self.select.items();
        // The button width is based on the longest button
        let width = items
            .iter()
            .map(|button| button.to_string().len())
            .max()
            .unwrap_or(0) as u16;
        let (areas, _) =
            Layout::horizontal(items.iter().map(|_| Constraint::Length(width)))
                .flex(Flex::SpaceAround)
                .split_with_spacers(metadata.area());

        for (button, area) in items.iter().zip(areas.iter()) {
            frame.render_widget(
                Button {
                    text: &button.to_string(),
                    has_focus: self.select.is_selected(button),
                }
                .generate(),
                *area,
            )
        }
    }
}
