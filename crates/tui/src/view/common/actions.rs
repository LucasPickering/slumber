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
    layout::{Constraint, Offset},
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
    /// Original action tree that this modal is derived from. This is stored in
    /// its original state
    items: Vec<MenuItem>,
    /// Stack of menu levels. Push when a group becomes visible, pop when it's
    /// hidden. The visual data is cloned from `self.items` so we can organize
    /// this in a stack. The original item tree doesn't allow that, and we
    /// can't use references to the tree because it would be self-referential.
    /// INVARIANT: len >= 1
    stack: Vec<Component<SelectState<MenuItemDisplay>>>,
    /// The index of the layer in the stack that the user is controlling. This
    /// index is always valid because the stack is never empty
    /// INVARIANT: selected_layer < self.stack.len()
    active_layer: usize,
}

impl ActionsModal {
    /// Width of a single layer of the menu
    const WIDTH: u16 = 30;

    /// Create a new actions modal, optional disabling certain actions based on
    /// some external condition(s).
    pub fn new(items: Vec<MenuItem>) -> Self {
        let root_select = build_select(map_items(&items));
        Self {
            items,
            stack: vec![root_select.into()],
            active_layer: 0,
        }
    }

    /// Clear all layers right of the given layer
    fn clear_children(&mut self, layer: usize) {
        self.stack.drain((layer + 1)..);
        // Defensive programming!!
        assert!(
            !self.stack.is_empty(),
            "Action menu stack must have at least one element"
        );
    }

    /// Open the children of a group in the active layer. This assumes the
    /// active layer is the top layer, so call [Self::clear_children] first.
    fn open_group(&mut self, items: Vec<MenuItemDisplay>) {
        self.stack.push(build_select(items).into());
    }

    /// Check if the given input action is bound to a menu action **in any
    /// layer**. This will start with the left-most layer and check each layer
    /// for an item bound to that action. If it's found, select that layer+item
    /// and return `true`. If not, return `false`.
    fn select_by_action(&mut self, action: Action) -> bool {
        for (layer, select) in self.stack.iter_mut().enumerate() {
            // Check if this input is bound to any item in this select
            let bound_index =
                select.data().items().position(|item| match item {
                    MenuItemDisplay::Action { shortcut, .. } => {
                        shortcut == &Some(action)
                    }
                    MenuItemDisplay::Group { .. } => false,
                });

            // This action is bound to something!
            if let Some(bound_index) = bound_index {
                self.active_layer = layer;
                select.data_mut().select_index(bound_index);
                return true;
            }
        }
        false
    }

    /// Select the previous layer to the left in the stack
    fn previous_layer(&mut self) {
        self.active_layer = self.active_layer.saturating_sub(1);
    }

    /// Select the next layer to the right in the stack
    fn next_layer(&mut self) {
        self.active_layer = (self.active_layer + 1).min(self.stack.len() - 1);
    }
}

impl Modal for ActionsModal {
    fn title(&self) -> Line<'_> {
        "Actions".into()
    }

    fn dimensions(&self) -> (Constraint, Constraint) {
        (
            Constraint::Length(Self::WIDTH),
            Constraint::Length(self.items.len() as u16),
        )
    }

    fn on_close(self: Box<Self>, submitted: bool) {
        if !submitted {
            return;
        }

        // To find the submitted action, we need to walk down the original
        // action tree in parallel with the stack. In each stack layer we have
        // a selected index, which we'll use to grab the next tree layer.
        let mut items = self.items;
        for i in 0..=self.active_layer {
            // Indexing is safe because selected_layer < stack.len()
            let select = self.stack[i].data();

            let Some(selected_index) = select.selected_index() else {
                return; // Possible if the final action menu is empty
            };
            let item = items.swap_remove(selected_index);
            if i < self.active_layer {
                // We have more layers to go - we're looking for a group
                let MenuItem::Group { children, .. } = item else {
                    panic!("Expected group at layer {i}, found {item:?}");
                };
                items = children;
            } else {
                // This is the last layer - we're looking for an action
                let MenuItem::Action(action) = item else {
                    panic!("Expected action at layer {i}, found {item:?}");
                };

                // Emit an event on behalf of the component that supplied this
                // action. The component will use its own supplied emitter ID to
                // consume the event
                action.emitter.emit(action.value);
            }
        }
    }
}

impl EventHandler for ActionsModal {
    fn update(&mut self, _: &mut UpdateContext, event: Event) -> Option<Event> {
        let mut propagated =
            event.opt().action(|action, propagate| match action {
                // Navigate between layers with left/right
                Action::Left => self.previous_layer(),
                Action::Right => self.next_layer(),
                _ => {
                    // Check if the input is bound to any action in *any* list
                    // in the stack
                    if self.select_by_action(action) {
                        // We need ownership of the menu action to emit it, so
                        // defer into the on_close handler. The relevant item
                        // will be selected so the handler knows what was
                        // submitted
                        self.close(true);
                    } else {
                        propagate.set();
                    }
                }
            });

        // Check for events from all layers of the stack. It's possible for
        // inactive layers to emit events because of mouse input (e.g.
        // scrolling).
        //
        // Iterate with indexes + while loop because we may modify the stack
        // while iterating. We need to recheck the bound after each iteration
        // to prevent iterating past the end when children are cleared
        let mut layer = 0;
        while layer < self.stack.len() {
            let emitter = self.stack[layer].to_emitter();
            propagated = propagated.emitted(emitter, |event| match event {
                SelectStateEvent::Select(index) => {
                    // When changing selection, any existing child menus are no
                    // longer relevant so close them
                    self.clear_children(layer);
                    // If the selected item is a group, open a new child menu
                    let selected = &self.stack[layer].data()[index];
                    if let MenuItemDisplay::Group { children, .. } = selected {
                        self.open_group(children.clone());
                    }
                }
                SelectStateEvent::Submit(index) => {
                    // Submitting on an action closes the menu and emits the
                    // action. Submitting on a group moves to the children
                    let selected = &self.stack[layer].data()[index];
                    match selected {
                        MenuItemDisplay::Action { .. } => self.close(true),
                        // The group should already be open because it had to be
                        // selected before it was submitted
                        MenuItemDisplay::Group { .. } => self.next_layer(),
                    }
                }
                SelectStateEvent::Toggle(_) => {}
            });
            layer += 1;
        }

        propagated
    }

    fn children(&mut self) -> Vec<Component<Child<'_>>> {
        // Children get priority. It shouldn't actually matter though because
        // only the active menu is focused so only that one gets key events,
        // and there's no visual overlap so they shouldn't be competing for
        // mouse events
        self.stack
            .iter_mut()
            .rev()
            .map(Component::to_child_mut)
            .collect()
    }
}

impl Draw for ActionsModal {
    fn draw(&self, frame: &mut Frame, (): (), metadata: DrawMetadata) {
        for (i, select) in self.stack.iter().enumerate() {
            // Each menu steps out to the right. We're intentionally blowing out
            // the frame of the modal
            let x = i32::from(Self::WIDTH) * i as i32;
            // Offset in y so our first item aligns with the parent, which is
            // the selected item from the previous layer
            let y = if i == 0 {
                0
            } else {
                self.stack[i - 1].data().selected_index().unwrap_or(0) as i32
            };
            let area = metadata.area().offset(Offset { x, y });

            // TODO fix styling for nested layers
            let is_active = i == self.active_layer;
            select.draw(
                frame,
                List::from(select.data()).active(is_active),
                area,
                is_active,
            );
        }
    }
}

/// An entry in an action menu
#[derive(Debug, derive_more::Display)]
pub enum MenuItem {
    /// A executable action
    #[display("{}", _0.name)]
    Action(MenuAction),
    /// A grouping of related actions, which can be opened in a nested menu
    #[display("{name}")]
    Group { name: String, children: Vec<Self> },
}

impl MenuItem {
    /// Is this menu item enabled?
    #[cfg(test)]
    pub fn enabled(&self) -> bool {
        match self {
            MenuItem::Action(action) => action.enabled,
            MenuItem::Group { .. } => true,
        }
    }
}

impl From<MenuAction> for MenuItem {
    fn from(value: MenuAction) -> Self {
        Self::Action(value)
    }
}

/// One item in an action menu modal. The action menu is built dynamically, and
/// each action is tied back to the component that supplied it via an [Emitter].
#[derive(Debug)]
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
}

/// Minimal version of [MenuItem] that can be cloned repeatedly to build
/// [SelectState]s
#[derive(Clone, Debug)]
enum MenuItemDisplay {
    Action {
        name: String,
        enabled: bool,
        shortcut: Option<Action>,
    },
    Group {
        name: String,
        children: Vec<Self>,
    },
}

impl Generate for &MenuItemDisplay {
    type Output<'this>
        = Span<'this>
    where
        Self: 'this;

    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this,
    {
        match self {
            MenuItemDisplay::Action { name, shortcut, .. } => {
                // If a shortcut is given, include the binding in the text
                shortcut
                    .map(|shortcut| {
                        TuiContext::get()
                            .input_engine
                            .add_hint(name, shortcut)
                            .into()
                    })
                    .unwrap_or_else(|| name.as_str().into())
            }
            MenuItemDisplay::Group { name, .. } => format!("{name} ▶").into(),
        }
    }
}

/// Map data tree to a tree that can be freely cloned and displayed
fn map_items(items: &[MenuItem]) -> Vec<MenuItemDisplay> {
    items
        .iter()
        .map(|item| match item {
            MenuItem::Action(action) => MenuItemDisplay::Action {
                name: action.name.clone(),
                enabled: action.enabled,
                shortcut: action.shortcut,
            },
            MenuItem::Group { name, children } => MenuItemDisplay::Group {
                name: name.clone(),
                // Recursion!
                children: map_items(children),
            },
        })
        .collect()
}

/// Build a select state from a list of menu items
fn build_select(items: Vec<MenuItemDisplay>) -> SelectState<MenuItemDisplay> {
    let disabled_indexes = items
        .iter()
        .enumerate()
        .filter(|(_, item)| {
            matches!(item, MenuItemDisplay::Action { enabled, .. } if !enabled)
        })
        .map(|(i, _)| i)
        .collect_vec();

    SelectState::builder(items)
        .disabled_indexes(disabled_indexes)
        .subscribe([SelectStateEventType::Select, SelectStateEventType::Submit])
        .build()
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
    struct Actionable {
        emitter: Emitter<TestAction>,
        /// List of returned actions is customizable for different test cases
        get_actions: Box<dyn Fn(Emitter<TestAction>) -> Vec<MenuItem>>,
    }

    impl Actionable {
        fn new(
            get_actions: impl 'static + Fn(Emitter<TestAction>) -> Vec<MenuItem>,
        ) -> Self {
            Self {
                emitter: Default::default(),
                get_actions: Box::new(get_actions),
            }
        }
    }

    impl Default for Actionable {
        fn default() -> Self {
            // By default, return all actions
            let get_actions = |emitter: Emitter<TestAction>| {
                vec![
                    // Disablify is first to test that disabled actions are
                    // skipped
                    emitter
                        .menu(TestAction::Disabled, "Disabled")
                        .enable(false)
                        .into(),
                    emitter.menu(TestAction::Action1, "Action 1").into(),
                    emitter.menu(TestAction::Action2, "Action 2").into(),
                    emitter
                        .menu(TestAction::Shortcutted, "Shortcutted")
                        .shortcut(Some(Action::Edit))
                        .into(),
                    MenuItem::Group {
                        name: "Nested".into(),
                        children: vec![
                            emitter
                                .menu(TestAction::Nested1, "Nested 1")
                                .into(),
                            emitter
                                .menu(TestAction::Nested2, "Nested 2")
                                .into(),
                            MenuItem::Group {
                                name: "Nested Group".into(),
                                children: vec![
                                    emitter
                                        .menu(
                                            TestAction::NestedNested1,
                                            "Nested Nested 1",
                                        )
                                        .into(),
                                ],
                            },
                        ],
                    },
                ]
            };
            Self::new(get_actions)
        }
    }

    impl EventHandler for Actionable {
        fn menu(&self) -> Vec<MenuItem> {
            (self.get_actions)(self.to_emitter())
        }
    }

    impl Draw for Actionable {
        fn draw(&self, _: &mut Frame, (): (), _: DrawMetadata) {}
    }

    impl ToEmitter<TestAction> for Actionable {
        fn to_emitter(&self) -> Emitter<TestAction> {
            self.emitter
        }
    }

    #[derive(Debug, PartialEq)]
    enum TestAction {
        Disabled,
        Action1,
        Action2,
        Shortcutted,
        // Second level!!
        Nested1,
        Nested2,
        // Third level!!
        NestedNested1,
    }

    /// Test basic action menu interactions
    #[rstest]
    fn test_actions(harness: TestHarness, terminal: TestTerminal) {
        let mut component =
            TestComponent::new(&harness, &terminal, Actionable::default());

        // Select a basic action
        component
            .int()
            .action("Action 2")
            .assert_emitted([TestAction::Action2]);

        // Actions can be selected by shortcut
        component
            .int()
            .send_keys([KeyCode::Char('x'), KeyCode::Char('e')])
            .assert_emitted([TestAction::Shortcutted]);
    }

    /// Various input sequences on multiple levels of nested actions
    #[rstest]
    // Navigate to the nested menu by arrow key
    #[case::right_arrow(
        &[KeyCode::Up, KeyCode::Right, KeyCode::Down, KeyCode::Enter],
        TestAction::Nested2,
    )]
    // Navigate back up to a parent layer with the left arrow
    #[case::left_arrow(
        &[KeyCode::Up, KeyCode::Right, KeyCode::Left, KeyCode::Up, KeyCode::Enter],
        TestAction::Shortcutted,
    )]
    // Navigate to the nested menu by Enter
    #[case::enter(
        &[KeyCode::Up, KeyCode::Enter, KeyCode::Enter],
        TestAction::Nested1,
    )]
    // Navigate to the innermost menu
    #[case::nested_nested(
        &[KeyCode::Up, KeyCode::Right, KeyCode::Up, KeyCode::Right, KeyCode::Enter],
        TestAction::NestedNested1,
    )]
    // Shortcuts should work regardless of which layer is active
    #[case::shortcut_from_other(
        &[KeyCode::Up, KeyCode::Right, KeyCode::Char('e')],
        TestAction::Shortcutted,
    )]
    fn test_actions_nested(
        harness: TestHarness,
        terminal: TestTerminal,
        #[case] inputs: &[KeyCode],
        #[case] expected_action: TestAction,
    ) {
        let mut component =
            TestComponent::new(&harness, &terminal, Actionable::default());
        component
            .int()
            .send_key(KeyCode::Char('x'))
            .send_keys(inputs.iter().copied())
            .assert_emitted([expected_action]);
    }

    /// There once was a bug where the select event wasn't handled correctly
    /// for the pre-selected item in the list. If the first item is a group, it
    /// should open correctly on first draw.
    #[rstest]
    fn test_first_group_selected(harness: TestHarness, terminal: TestTerminal) {
        let get_actions = |emitter: Emitter<TestAction>| {
            vec![MenuItem::Group {
                name: "Nested".into(),
                children: vec![
                    emitter.menu(TestAction::Nested1, "Nested 1").into(),
                    emitter.menu(TestAction::Nested2, "Nested 2").into(),
                ],
            }]
        };

        let mut component = TestComponent::new(
            &harness,
            &terminal,
            Actionable::new(get_actions),
        );

        // Group should be expanded when the modal is first opened
        component
            .int()
            .send_keys([KeyCode::Char('x'), KeyCode::Right, KeyCode::Enter])
            .assert_emitted([TestAction::Nested1]);
    }
}
