use crate::view::{
    common::{list::List, modal::Modal},
    component::Component,
    draw::{Draw, DrawMetadata, Generate},
    event::{Event, EventHandler},
    state::fixed_select::{FixedSelect, FixedSelectState},
    ViewContext,
};
use ratatui::{
    layout::Constraint,
    text::{Line, Span},
    widgets::ListState,
    Frame,
};

/// Modal to list and trigger arbitrary actions. The list of available actions
/// is defined by the generic parameter
#[derive(Debug)]
pub struct ActionsModal<T: FixedSelect> {
    /// Join the list of global actions into the given one
    actions: Component<FixedSelectState<T, ListState>>,
}

impl<T: FixedSelect> ActionsModal<T> {
    /// Create a new actions modal, optionall disabling certain actions based on
    /// some external condition(s).
    pub fn new(disabled_actions: &[T]) -> Self {
        let on_submit = move |action: &mut T| {
            // Close the modal *first*, so the parent can handle the
            // callback event. Jank but it works
            ViewContext::push_event(Event::CloseModal);
            ViewContext::push_event(Event::new_local(*action));
        };

        Self {
            actions: FixedSelectState::builder()
                .disabled_items(disabled_actions)
                .on_submit(on_submit)
                .build()
                .into(),
        }
    }
}

impl<T: FixedSelect> Default for ActionsModal<T> {
    fn default() -> Self {
        Self::new(&[])
    }
}

impl<T> Modal for ActionsModal<T>
where
    T: FixedSelect,
    ActionsModal<T>: Draw,
{
    fn title(&self) -> Line<'_> {
        "Actions".into()
    }

    fn dimensions(&self) -> (Constraint, Constraint) {
        (Constraint::Length(30), Constraint::Length(T::COUNT as u16))
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
    fn draw(&self, frame: &mut Frame, _: (), metadata: DrawMetadata) {
        self.actions.draw(
            frame,
            List::from(self.actions.data()),
            metadata.area(),
            true,
        );
    }
}
