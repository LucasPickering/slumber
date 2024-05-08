use crate::{
    tui::view::{
        common::{list::List, modal::Modal},
        component::Component,
        draw::{Draw, Generate, ToStringGenerate},
        event::{Event, EventHandler, EventQueue},
        state::fixed_select::{FixedSelectState, FixedSelectWithoutDefault},
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
pub struct ActionsModal<T: FixedSelectWithoutDefault = EmptyAction> {
    /// Join the list of global actions into the given one
    actions: Component<FixedSelectState<EnumChain<GlobalAction, T>, ListState>>,
}

impl<T: FixedSelectWithoutDefault> Default for ActionsModal<T> {
    fn default() -> Self {
        let on_submit = move |action: &mut EnumChain<GlobalAction, T>| {
            // Close the modal *first*, so the parent can handle the
            // callback event. Jank but it works
            EventQueue::push(Event::CloseModal);
            let event = match action {
                EnumChain::T(action) => Event::new_other(*action),
                EnumChain::U(action) => Event::new_other(*action),
            };
            EventQueue::push(event);
        };

        Self {
            actions: FixedSelectState::builder()
                .on_submit(on_submit)
                .build()
                .into(),
        }
    }
}

impl<T> Modal for ActionsModal<T>
where
    T: FixedSelectWithoutDefault,
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

impl<T: FixedSelectWithoutDefault> EventHandler for ActionsModal<T> {
    fn children(&mut self) -> Vec<Component<&mut dyn EventHandler>> {
        vec![self.actions.as_child()]
    }
}

impl<T> Draw for ActionsModal<T>
where
    T: 'static + FixedSelectWithoutDefault,
    for<'a> &'a T: Generate<Output<'a> = Span<'a>>,
{
    fn draw(&self, frame: &mut Frame, _: (), area: Rect) {
        self.actions.draw(
            frame,
            List {
                pane: None,
                list: self.actions.data().items(),
            }
            .generate(),
            area,
        );
    }
}

/// Actions that appear in all action modals
#[derive(
    Copy, Clone, Debug, Default, Display, EnumCount, EnumIter, PartialEq,
)]
pub enum GlobalAction {
    #[default]
    #[display("Edit Collection")]
    EditCollection,
}

impl ToStringGenerate for GlobalAction {}

/// Empty action list. Used when only the default global actions should be shown
#[derive(Copy, Clone, Debug, Display, EnumCount, EnumIter, PartialEq)]
pub enum EmptyAction {}

impl ToStringGenerate for EmptyAction {}
