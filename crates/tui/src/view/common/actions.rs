use crate::{
    context::TuiContext,
    view::{
        common::{list::List, modal::Modal},
        component::Component,
        context::UpdateContext,
        draw::{Draw, DrawMetadata, Generate},
        event::{
            Child, Emitter, Event, EventHandler, LocalEvent, OptionEvent,
            ToEmitter,
        },
        state::select::{SelectState, SelectStateEvent, SelectStateEventType},
    },
};
use itertools::Itertools;
use ratatui::{
    Frame,
    layout::Constraint,
    text::{Line, Span},
};
use slumber_config::Action;

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
            let Some(action) = self.actions.into_data().into_selected() else {
                // Possible if the action list is empty
                return;
            };
            // Emit an event on behalf of the component that supplied this
            // action. The component will use its own supplied emitter ID to
            // consume the event
            action.emitter.emit(action.value);
        }
    }
}

impl EventHandler for ActionsModal {
    fn update(&mut self, _: &mut UpdateContext, event: Event) -> Option<Event> {
        event
            .opt()
            .action(|action, propagate| {
                // For any input action, check if any menu items are bound to it
                // as a shortcut. If there are multiple menu actions bound to
                // the same shortcut, we'll just take the first.
                let bound_index =
                    self.actions.data().items().position(|menu_action| {
                        menu_action.shortcut == Some(action)
                    });
                if let Some(index) = bound_index {
                    // We need ownership of the menu action to emit it, so defer
                    // into the on_close handler. Selecting the item is how we
                    // know which one to submit
                    self.actions.data_mut().select_index(index);
                    self.close(true);
                } else {
                    propagate.set();
                }
            })
            .emitted(self.actions.to_emitter(), |event| {
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
    fn draw(&self, frame: &mut Frame, (): (), metadata: DrawMetadata) {
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
        view::test_util::TestComponent,
    };
    use rstest::rstest;
    use terminput::KeyCode;

    /// A component that provides some actions
    #[derive(Default)]
    struct Actionable {
        emitter: Emitter<TestMenuAction>,
    }

    impl EventHandler for Actionable {
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
        fn draw(&self, _: &mut Frame, (): (), _: DrawMetadata) {}
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
