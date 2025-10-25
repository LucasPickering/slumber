//! Buttons and button accessories

use crate::{
    context::TuiContext,
    view::{
        Generate,
        component::{Canvas, Component, ComponentId, Draw, DrawMetadata},
        context::UpdateContext,
        event::{Emitter, Event, OptionEvent, ToEmitter},
        state::fixed_select::{FixedSelect, FixedSelectState},
    },
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
    id: ComponentId,
    /// The only type of event we can emit is a button being selected, so just
    /// emit the button type
    emitter: Emitter<T>,
    select: FixedSelectState<T>,
}

impl<T: FixedSelect> Component for ButtonGroup<T> {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn update(&mut self, _: &mut UpdateContext, event: Event) -> Option<Event> {
        event.opt().action(|action, propagate| match action {
            Action::Left => self.select.previous(),
            Action::Right => self.select.next(),
            Action::Submit => {
                // Propagate the selected item as a dynamic event
                self.emitter.emit(self.select.selected());
            }
            _ => propagate.set(),
        })
    }

    // Do *not* treat the select state as a child, because the default select
    // action bindings aren't intuitive for this component
}

impl<T: FixedSelect> Draw for ButtonGroup<T> {
    fn draw_impl(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
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

impl<T: FixedSelect> ToEmitter<T> for ButtonGroup<T> {
    fn to_emitter(&self) -> Emitter<T> {
        self.emitter
    }
}
