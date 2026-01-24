//! Buttons and button accessories

use crate::view::{
    Generate,
    common::fixed_select::{FixedSelect, FixedSelectItem},
    component::{Canvas, Component, ComponentId, Draw, DrawMetadata},
    context::{UpdateContext, ViewContext},
    event::{Event, EventMatch},
};
use ratatui::{
    layout::{Constraint, Flex, Layout},
    text::Span,
};
use slumber_config::Action;

/// An piece of text that the user can "press" with the submit action. It should
/// only be interactable if it is focused, but that's up to the caller to
/// enforce.
pub struct Button<'a> {
    text: &'a str,
    has_focus: bool,
}

impl Generate for Button<'_> {
    type Output<'this>
        = Span<'this>
    where
        Self: 'this;

    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this,
    {
        let styles = ViewContext::styles();
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
/// activate one.
///
/// This does **not** listen for submission events; the user is responsible for
/// listening and checking which button is selected at that time. This makes it
/// easier to use in modals, where the modal queue listens for submission.
#[derive(Debug, Default)]
pub struct ButtonGroup<T: FixedSelectItem> {
    id: ComponentId,
    select: FixedSelect<T>,
}

impl<T: FixedSelectItem> ButtonGroup<T> {
    pub fn selected(&self) -> T {
        self.select.selected()
    }
}

impl<T: FixedSelectItem> Component for ButtonGroup<T> {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn update(&mut self, _: &mut UpdateContext, event: Event) -> EventMatch {
        event.m().action(|action, propagate| match action {
            Action::Left => self.select.previous(),
            Action::Right => self.select.next(),
            _ => propagate.set(),
        })
    }

    // Do *not* treat the select state as a child, because the default select
    // action bindings aren't intuitive for this component
}

impl<T: FixedSelectItem> Draw for ButtonGroup<T> {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        let (areas, _) =
            Layout::horizontal(self.select.items().map(|button| {
                Constraint::Length(button.to_string().len() as u16)
            }))
            .flex(Flex::SpaceBetween)
            .split_with_spacers(metadata.area());

        for (button, area) in self.select.items().zip(areas.iter()) {
            canvas.render_widget(
                Button {
                    text: &button.to_string(),
                    has_focus: self.select.is_selected(button),
                }
                .generate(),
                *area,
            );
        }
    }
}
