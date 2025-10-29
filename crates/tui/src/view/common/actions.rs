use crate::{
    context::TuiContext,
    view::{
        Generate,
        common::{
            list::List,
            modal::{Modal, ModalEvent},
        },
        component::{
            Canvas, Child, Component, ComponentId, Draw, DrawMetadata, ToChild,
        },
        context::UpdateContext,
        event::{Emitter, Event, LocalEvent, OptionEvent},
        state::select::SelectState,
    },
};
use itertools::Itertools;
use ratatui::{
    layout::Constraint,
    text::{Line, Span},
};
use slumber_config::Action;

/// Modal to list and trigger arbitrary actions. The user opens the action menu
/// with a keybinding, at which point the list of available actions is built
/// dynamically via [Component::menu_actions]. When an action is selected,
/// the modal is closed and that action will be emitted as a dynamic event, to
/// be handled by the component that originally supplied it. Each component that
/// provides actions should store an [Emitter] specifically for its actions,
/// which will be provided to each supplied action and can be used to check and
/// consume the action events.
#[derive(Debug)]
pub struct ActionsModal {
    id: ComponentId,
    /// Join the list of global actions into the given one
    actions: SelectState<MenuAction>,
    /// Emit modal close events back to the parent
    modal_emitter: Emitter<ModalEvent>,
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
            id: ComponentId::default(),
            actions: SelectState::builder(actions)
                .disabled_indexes(disabled_indexes)
                .build(),
            modal_emitter: Emitter::default(),
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
            Constraint::Length(self.actions.len() as u16),
        )
    }

    fn emitter(&self) -> Option<Emitter<ModalEvent>> {
        Some(self.modal_emitter)
    }

    fn on_submit(self, _: &mut UpdateContext) {
        let Some(action) = self.actions.into_selected() else {
            // Possible if the action list is empty
            return;
        };
        // Emit an event on behalf of the component that supplied this
        // action. The component will use its own supplied emitter ID to
        // consume the event
        action.emitter.emit(action.value);
    }
}

impl Component for ActionsModal {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn update(&mut self, _: &mut UpdateContext, event: Event) -> Option<Event> {
        // Enter submission is handled by the modal parent. Hotkey submission
        // requires extra logic
        event.opt().action(|action, propagate| {
            // For any input action, check if any menu items are bound to it
            // as a shortcut. If there are multiple menu actions bound to
            // the same shortcut, we'll just take the first.
            let bound_index = self
                .actions
                .items()
                .position(|menu_action| menu_action.shortcut == Some(action));
            if let Some(index) = bound_index {
                // We need ownership of the menu action to emit it, so defer
                // into the on_submit handler. Selecting the item is how we
                // know which one to submit
                self.actions.select_index(index);
                // Normally the modal queue parent listens for the Enter key
                // to know when a submission occurs. Since this is a hotkey
                // submission, we have to explicitly tell it
                self.modal_emitter.emit(ModalEvent::Submit);
            } else {
                propagate.set();
            }
        })
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        vec![self.actions.to_child_mut()]
    }
}

impl Draw for ActionsModal {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        canvas.draw(
            &self.actions,
            List::from(&self.actions),
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
    /// Input action bound to this menu action
    shortcut: Option<Action>,
}

impl MenuAction {
    /// Create a new menu action. This uses the builder-lite pattern to
    /// customize the created event
    pub fn new<T: LocalEvent>(
        emitter: Emitter<T>,
        action: T,
        name: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            emitter: emitter.upcast(),
            enabled: true,
            shortcut: None,
            value: Box::new(action),
        }
    }

    /// Enable/disable this action
    pub fn enable(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Set/clear the shortcut for this action
    pub fn shortcut(mut self, shortcut: Option<Action>) -> Self {
        self.shortcut = shortcut;
        self
    }

    /// Is this action enabled?
    #[cfg(test)]
    pub fn enabled(&self) -> bool {
        self.enabled
    }
}

impl Generate for &MenuAction {
    type Output<'this>
        = Span<'this>
    where
        Self: 'this;

    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this,
    {
        // If a shortcut is given, include the binding in the text
        self.shortcut
            .map(|shortcut| {
                TuiContext::get()
                    .input_engine
                    .add_hint(&self.name, shortcut)
                    .into()
            })
            .unwrap_or_else(|| self.name.as_str().into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_util::{TestHarness, TestTerminal, harness, terminal},
        view::{event::ToEmitter, test_util::TestComponent},
    };
    use rstest::rstest;
    use terminput::KeyCode;

    /// A component that provides some actions
    #[derive(Debug, Default)]
    struct Actionable {
        id: ComponentId,
        emitter: Emitter<TestMenuAction>,
    }

    impl Component for Actionable {
        fn id(&self) -> ComponentId {
            self.id
        }

        fn menu_actions(&self) -> Vec<MenuAction> {
            let emitter = self.emitter;
            vec![
                emitter.menu(TestMenuAction::Flobrigate, "Flobrigate"),
                emitter.menu(TestMenuAction::Profilate, "Profilate"),
                emitter
                    .menu(TestMenuAction::Disablify, "Disablify")
                    .enable(false),
                emitter
                    .menu(TestMenuAction::Shortcutticated, "Shortcutticated")
                    .shortcut(Some(Action::Edit)),
            ]
        }
    }

    impl Draw for Actionable {
        fn draw(&self, _: &mut Canvas, (): (), _: DrawMetadata) {}
    }

    impl ToEmitter<TestMenuAction> for Actionable {
        fn to_emitter(&self) -> Emitter<TestMenuAction> {
            self.emitter
        }
    }

    #[derive(Debug, PartialEq)]
    enum TestMenuAction {
        // Disablify is first to test that disabled actions are skipped
        Disablify,
        Flobrigate,
        Profilate,
        Shortcutticated,
    }

    /// Test basic action menu interactions
    #[rstest]
    fn test_actions(harness: TestHarness, terminal: TestTerminal) {
        let mut component =
            TestComponent::new(&harness, &terminal, Actionable::default());

        // Select a basic action
        component
            .int()
            .action("Profilate")
            .assert_emitted([TestMenuAction::Profilate]);

        // Actions can be selected by shortcut
        component
            .int()
            .send_keys([KeyCode::Char('x'), KeyCode::Char('e')])
            .assert_emitted([TestMenuAction::Shortcutticated]);
    }
}
