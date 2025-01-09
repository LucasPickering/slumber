use crate::view::{
    common::{list::List, modal::Modal},
    component::Component,
    context::UpdateContext,
    draw::{Draw, DrawMetadata, ToStringGenerate},
    event::{
        Child, Emitter, Event, EventHandler, LocalEvent, OptionEvent, ToEmitter,
    },
    state::select::{SelectState, SelectStateEvent, SelectStateEventType},
};
use itertools::Itertools;
use ratatui::{layout::Constraint, text::Line, Frame};
use std::fmt::Display;
use strum::IntoEnumIterator;

/// Modal to list and trigger arbitrary actions. The user opens the action menu
/// with a keybinding, at which point the list of available actions is built
/// dynamically via [EventHandler::menu_actions]. When an action is selected,
/// the modal is closed and that action will be emitted as a dynamic event, to
/// be handled by the component that originally supplied it. Each component that
/// provides actions should store an [Emitter] specifically for its actions,
/// which will be provided to each supplied action and can be used to check and
/// consume the action events.
#[derive(Debug)]
pub struct ActionsModal {
    /// Join the list of global actions into the given one
    actions: Component<SelectState<MenuAction>>,
}

impl ActionsModal {
    /// Create a new actions modal, optional disabling certain actions based on
    /// some external condition(s).
    pub fn new(actions: Vec<MenuAction>) -> Self {
        let disabled_indexes = actions
            .iter()
            .enumerate()
            .filter(|(_, action)| !action.enabled)
            .map(|(i, _)| i)
            .collect_vec();
        Self {
            actions: SelectState::builder(actions)
                .disabled_indexes(disabled_indexes)
                .subscribe([SelectStateEventType::Submit])
                .build()
                .into(),
        }
    }
}

impl Modal for ActionsModal {
    fn title(&self) -> Line<'_> {
        "Actions".into()
    }

    fn dimensions(&self) -> (Constraint, Constraint) {
        (
            Constraint::Length(30),
            Constraint::Length(self.actions.data().len() as u16),
        )
    }

    fn on_close(self: Box<Self>, submitted: bool) {
        if submitted {
            let action = self
                .actions
                .into_data()
                .into_selected()
                .expect("User submitted something");
            // Emit an event on behalf of the component that supplied this
            // action. The component will use its own supplied emitter ID to
            // consume the event
            action.emitter.emit(action.value);
        }
    }
}

impl EventHandler for ActionsModal {
    fn update(&mut self, _: &mut UpdateContext, event: Event) -> Option<Event> {
        event.opt().emitted(self.actions.to_emitter(), |event| {
            if let SelectStateEvent::Submit(_) = event {
                self.close(true);
            }
        })
    }

    fn children(&mut self) -> Vec<Component<Child<'_>>> {
        vec![self.actions.to_child_mut()]
    }
}

impl Draw for ActionsModal {
    fn draw(&self, frame: &mut Frame, _: (), metadata: DrawMetadata) {
        self.actions.draw(
            frame,
            List::from(self.actions.data()),
            metadata.area(),
            true,
        );
    }
}

/// One item in an action menu modal. The action menu is built dynamically, and
/// each action is tied back to the component that supplied it via an [Emitter].
#[derive(Debug, derive_more::Display)]
#[display("{name}")]
pub struct MenuAction {
    name: String,
    value: Box<dyn LocalEvent>,
    /// Because actions are sourced from multiple components, we use a
    /// type-erased emitter here. When the action is selected, we'll emit it on
    /// behalf of the supplier, who will then downcast and consume it in its
    /// update() handler.
    emitter: Emitter<dyn LocalEvent>,
    enabled: bool,
}

impl ToStringGenerate for MenuAction {}

/// Trait for an enum that can be converted into a list of actions. Most
/// components have a static list of actions available, so this trait makes it
/// easy to implement [EventHandler::menu_actions].
pub trait IntoMenuActions<Data>:
    Display + IntoEnumIterator + LocalEvent
{
    /// Create a list of actions, one per variant in this enum
    fn into_actions(data: &Data) -> Vec<MenuAction>
    where
        Data: ToEmitter<Self>,
    {
        Self::iter()
            .map(|action| MenuAction {
                name: action.to_string(),
                enabled: action.enabled(data),
                emitter: data.to_emitter().upcast(),
                value: Box::new(action),
            })
            .collect()
    }

    /// Should this action be enabled in the menu?
    fn enabled(&self, data: &Data) -> bool;
}
