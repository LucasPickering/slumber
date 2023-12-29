use crate::tui::view::{
    common::{list::List, modal::Modal},
    component::Component,
    draw::{Draw, Generate},
    event::{Event, EventHandler, UpdateContext},
    state::select::{Fixed, FixedSelect, SelectState},
};
use ratatui::{
    layout::{Constraint, Rect},
    text::Span,
    widgets::ListState,
    Frame,
};

/// Modal to list and trigger arbitrary actions. The list of available actions
/// is defined by the generic parameter
#[derive(Debug)]
pub struct ActionsModal<T: FixedSelect> {
    actions: Component<SelectState<Fixed, T, ListState>>,
}

impl<T: FixedSelect> Default for ActionsModal<T> {
    fn default() -> Self {
        let wrapper = move |context: &mut UpdateContext, action: &mut T| {
            // Close the modal *first*, so the parent can handle the callback
            // event. Jank but it works
            context.queue_event(Event::CloseModal);
            context.queue_event(Event::other(*action));
        };

        Self {
            actions: SelectState::fixed().on_submit(wrapper).into(),
        }
    }
}

impl<T> Modal for ActionsModal<T>
where
    T: FixedSelect,
    ActionsModal<T>: Draw,
{
    fn title(&self) -> &str {
        "Actions"
    }

    fn dimensions(&self) -> (Constraint, Constraint) {
        (
            Constraint::Length(30),
            Constraint::Length(T::iter().count() as u16),
        )
    }
}

impl<T: FixedSelect> EventHandler for ActionsModal<T> {
    fn children(&mut self) -> Vec<Component<&mut dyn EventHandler>> {
        vec![self.actions.as_child()]
    }
}

impl<T> Draw for ActionsModal<T>
where
    T: 'static + FixedSelect,
    for<'a> &'a T: Generate<Output<'a> = Span<'a>>,
{
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
