use crate::{
    tui::view::{
        common::{list::List, modal::Modal},
        component::Component,
        draw::{Draw, Generate, ToStringGenerate},
        event::{Event, EventHandler, UpdateContext},
        state::select::{Fixed, FixedSelect, SelectState},
    },
    util::EnumChain,
};
use derive_more::Display;
use ratatui::{
    layout::{Constraint, Rect},
    text::Span,
    widgets::ListState,
    Frame,
};
use strum::{EnumCount, EnumIter};

/// Modal to list and trigger arbitrary actions. The list of available actions
/// is defined by the generic parameter
#[derive(Debug)]
pub struct ActionsModal<T: FixedSelect = EmptyAction> {
    /// Join the list of global actions into the given one
    actions:
        Component<SelectState<Fixed, EnumChain<GlobalAction, T>, ListState>>,
}

impl<T: FixedSelect> Default for ActionsModal<T> {
    fn default() -> Self {
        let wrapper =
            move |context: &mut UpdateContext,
                  action: &mut EnumChain<GlobalAction, T>| {
                // Close the modal *first*, so the parent can handle the
                // callback event. Jank but it works
                context.queue_event(Event::CloseModal);
                let event = match action {
                    EnumChain::T(action) => Event::other(*action),
                    EnumChain::U(action) => Event::other(*action),
                };
                context.queue_event(event);
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
            Constraint::Length(EnumChain::<GlobalAction, T>::COUNT as u16),
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

/// Actions that appear in all action modals
#[derive(Copy, Clone, Debug, Display, EnumCount, EnumIter, PartialEq)]
pub enum GlobalAction {
    #[display("Edit Collection")]
    EditCollection,
}

impl ToStringGenerate for GlobalAction {}

/// Empty action list. Used when only the default global actions should be shown
#[derive(Copy, Clone, Debug, Display, EnumCount, EnumIter, PartialEq)]
pub enum EmptyAction {}

impl ToStringGenerate for EmptyAction {}
