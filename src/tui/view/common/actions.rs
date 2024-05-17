use crate::{
    tui::view::{
        common::{list::List, modal::Modal},
        component::Component,
        draw::{Draw, DrawMetadata, Generate, ToStringGenerate},
        event::{Event, EventHandler},
        state::fixed_select::{FixedSelectState, FixedSelectWithoutDefault},
        ViewContext,
    },
    util::EnumChain,
};
use derive_more::Display;
use ratatui::{
    layout::Constraint,
    text::{Line, Span},
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
            ViewContext::push_event(Event::CloseModal);
            let event = match action {
                EnumChain::T(action) => Event::new_other(*action),
                EnumChain::U(action) => Event::new_other(*action),
            };
            ViewContext::push_event(event);
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
    fn title(&self) -> Line<'_> {
        "Actions".into()
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
    fn draw(&self, frame: &mut Frame, _: (), metadata: DrawMetadata) {
        self.actions.draw(
            frame,
            List::new(self.actions.data().items()),
            metadata.area(),
            true,
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
