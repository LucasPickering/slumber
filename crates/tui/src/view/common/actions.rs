use crate::view::{
    common::{list::List, modal::Modal},
    component::Component,
    context::UpdateContext,
    draw::{Draw, DrawMetadata, Generate},
    event::{Child, Emitter, EmitterId, Event, EventHandler, Update},
    state::{
        fixed_select::{FixedSelect, FixedSelectState},
        select::{SelectStateEvent, SelectStateEventType},
    },
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
    emitter_id: EmitterId,
    /// Join the list of global actions into the given one
    actions: Component<FixedSelectState<T, ListState>>,
}

impl<T: FixedSelect> ActionsModal<T> {
    /// Create a new actions modal, optional disabling certain actions based on
    /// some external condition(s).
    pub fn new(disabled_actions: &[T]) -> Self {
        Self {
            emitter_id: EmitterId::new(),
            actions: FixedSelectState::builder()
                .disabled_items(disabled_actions)
                .subscribe([SelectStateEventType::Submit])
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

impl<T> EventHandler for ActionsModal<T>
where
    T: FixedSelect,
    ActionsModal<T>: Draw,
{
    fn update(&mut self, _: &mut UpdateContext, event: Event) -> Update {
        event.m().emitted(self.actions.handle(), |event| {
            if let SelectStateEvent::Submit(index) = event {
                // Close modal first so the parent can consume the emitted
                // event
                self.close(true);
                let action = self.actions.data()[index];
                self.emit(action);
            }
        })
    }

    fn children(&mut self) -> Vec<Component<Child<'_>>> {
        vec![self.actions.to_child_mut()]
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

impl<T: FixedSelect> Emitter for ActionsModal<T> {
    /// Emit the action itself
    type Emitted = T;

    fn id(&self) -> EmitterId {
        self.emitter_id
    }
}
