use crate::{
    context::TuiContext,
    view::{
        Generate,
        component::{
            Canvas, Child, Component, ComponentId, Draw, DrawMetadata, Portal,
            ToChild,
        },
        context::UpdateContext,
        event::{Emitter, Event, EventMatch, LocalEvent, ToEmitter},
        state::select::{
            Select, SelectEvent, SelectEventType, SelectListProps,
        },
    },
};
use itertools::Itertools;
use ratatui::{
    layout::{Constraint, Position},
    prelude::Rect,
    text::Span,
};
use slumber_config::Action;
use std::fmt::{self, Display};
use unicode_width::UnicodeWidthStr;

/// Popup menu to list and trigger arbitrary actions.
///
/// The user opens the action menu with a keybinding, at which point the list of
/// available actions is built dynamically via [Component::menu]. When an action
/// is selected, the modal is closed and that action will be emitted as a
/// dynamic event, to be handled by the component that originally supplied it.
/// Each component that provides actions should store an [Emitter] specifically
/// for its actions, which will be provided to each supplied action and can be
/// used to check and consume the action events.
///
/// This is implemented as its own [Portal] type instead of using
/// [ModalQueue](super::modal::ModalQueue) because the behavior is sufficiently
/// different:
/// - It doesn't use the modal's standard border styling
/// - The location isn't necessarily centered
/// - The event handling is more complex (indirect submission)
#[derive(Debug, Default)]
pub struct ActionMenu {
    id: ComponentId,
    /// Menu content, which is `Some` when the menu is open
    content: Option<ActionMenuContent>,
}

impl ActionMenu {
    /// Open the actions menu with the given actions/groups
    pub fn open(&mut self, items: Vec<MenuItem>) {
        self.content = Some(ActionMenuContent::new(items));
    }

    fn close(&mut self) {
        self.content = None;
    }
}

impl Portal for ActionMenu {
    fn area(&self, canvas_area: Rect) -> Rect {
        let Some(content) = &self.content else {
            return Rect::default();
        };

        // Center just based on the first layer, so it doesn't shift when
        // opening other layers
        let first = content.stack.first().expect("Menu stack cannot be empty");
        let Rect { x, y, .. } = canvas_area.centered(
            Constraint::Length(content.layer_width(&content.stack[0])),
            Constraint::Length(first.len() as u16),
        );

        // Each layer has a dynamic width, so sum them up
        let width = content
            .stack
            .iter()
            .map(|layer| content.layer_width(layer))
            .sum();

        // Calculate how far down the menus expand. Each menu is offset so that
        // the first item lines up with the selected item in the parent
        let mut offset_y = 0;
        let height = content
            .stack
            .iter()
            .map(|layer| {
                let layer_height = offset_y + layer.len() as u16;
                offset_y += layer.selected_index().unwrap_or(0) as u16;
                layer_height
            })
            .max()
            .unwrap_or(0);

        Rect {
            x,
            y,
            width,
            height,
        }
    }
}

impl Component for ActionMenu {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn update(&mut self, _: &mut UpdateContext, event: Event) -> EventMatch {
        // Don't eat events until we're open
        let Some(content) = &self.content else {
            return event.m();
        };
        let emitter = content.emitter;

        event
            .m()
            .action(|action, propagate| match action {
                Action::Cancel | Action::Quit => self.close(),
                _ => propagate.set(),
            })
            .emitted(
                emitter,
                // Unwraps are safe because we can only get an event if the
                // content exists
                |_: ActionSubmit| self.content.take().unwrap().submit(),
            )
            .any(|event| match event {
                // Eat any input events, since we're the sole focus holder
                Event::Input { .. } => None,
                _ => Some(event),
            })
    }

    fn contains(&self, _position: Position) -> bool {
        // We want to receive clicks in the background, but we can't draw to
        // that space or it would actually cover everything up
        true
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        vec![self.content.to_child_mut()]
    }
}

impl Draw for ActionMenu {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        if let Some(state) = &self.content {
            canvas.draw(state, (), metadata.area(), true);
        }
    }
}

/// Data for a particular action menu. This is created when the menu is opened
/// based on what actions are available
#[derive(Debug)]
struct ActionMenuContent {
    id: ComponentId,
    /// Original action tree that this modal is derived from. This is stored in
    /// its original state
    items: Vec<MenuItem>,
    /// Stack of menu levels. Push when a group becomes visible, pop when it's
    /// hidden. The visual data is cloned from `self.items` so we can organize
    /// this in a stack. The original item tree doesn't allow that, and we
    /// can't use references to the tree because it would be self-referential.
    /// INVARIANT: len >= 1
    stack: Vec<Select<MenuItemDisplay>>,
    /// The index of the layer in the stack that the user is controlling. This
    /// index is always valid because the stack is never empty
    /// INVARIANT: selected_layer < self.stack.len()
    active_layer: usize,
    /// Emitter to tell the parent when we're executing an action. Submission
    /// requires an owned `self`, so it has to be done as the parent closes us
    emitter: Emitter<ActionSubmit>,
}

impl ActionMenuContent {
    fn new(items: Vec<MenuItem>) -> Self {
        let root_select = build_select(map_items(&items));
        Self {
            id: ComponentId::default(),
            items,
            stack: vec![root_select],
            active_layer: 0,
            emitter: Emitter::default(),
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
        self.stack.push(build_select(items));
    }

    /// Check if the given input action is bound to a menu action **in any
    /// layer**. This will start with the left-most layer and check each layer
    /// for an item bound to that action. If it's found, select that layer+item
    /// and return `true`. If not, return `false`.
    fn select_by_action(&mut self, action: Action) -> bool {
        for (layer, select) in self.stack.iter_mut().enumerate() {
            // Check if this input is bound to any item in this select
            let bound_index = select.items().position(|item| match item {
                MenuItemDisplay::Action { shortcut, .. } => {
                    shortcut == &Some(action)
                }
                MenuItemDisplay::Group { .. } => false,
            });

            // This action is bound to something!
            if let Some(bound_index) = bound_index {
                self.active_layer = layer;
                select.select_index(bound_index);
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

    /// Perform the selected action
    fn submit(self) {
        // To find the submitted action, we need to walk down the original
        // action tree in parallel with the stack. In each stack layer we have
        // a selected index, which we'll use to grab the next tree layer.
        let mut items = self.items;
        for i in 0..=self.active_layer {
            // Indexing is safe because active_layer < stack.len()
            let select = &self.stack[i];

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

    /// Get the pixel width of a particular layer
    fn layer_width(&self, layer: &Select<MenuItemDisplay>) -> u16 {
        // Get the longest item
        layer
            .items()
            .map(|item| item.to_string().width() as u16)
            .max()
            .unwrap_or(0)
            + 1 // Padding between layers
    }
}

impl Component for ActionMenuContent {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn update(&mut self, _: &mut UpdateContext, event: Event) -> EventMatch {
        let mut propagated = event
            .m()
            .click(|position, propagate| {
                // Select clicked layer
                for (i, layer) in self.stack.iter().enumerate() {
                    if layer.contains(position) {
                        self.active_layer = i;
                    }
                }
                // Clicks are propagated by default, but we want to EAT THEM ALL
                propagate.unset();
            })
            .action(|action, propagate| match action {
                // Navigate between layers with left/right
                Action::Left => self.previous_layer(),
                Action::Right => self.next_layer(),
                // Check if the input is bound to any action in *any* list in
                // the stack
                _ if self.select_by_action(action) => {
                    // Submission is deferred because it requires an
                    // owned value
                    self.emitter.emit(ActionSubmit);
                }
                _ => propagate.set(),
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
                SelectEvent::Select(index) => {
                    // When changing selection, any existing child menus are no
                    // longer relevant so close them
                    self.clear_children(layer);
                    // If the selected item is a group, open a new child menu
                    let selected = &self.stack[layer][index];
                    if let MenuItemDisplay::Group { children, .. } = selected {
                        self.open_group(children.clone());
                    }
                }
                SelectEvent::Submit(index) => {
                    // Submitting on an action closes the menu and emits the
                    // action. Submitting on a group moves to the children
                    //
                    // We have to handle the submission event here instead of
                    // letting the modal queue handle it because **not all
                    // submissions close the modal**; submission on a group
                    // just enters the next layer
                    let selected = &self.stack[layer][index];
                    match selected {
                        MenuItemDisplay::Action { .. } => {
                            // Submission is deferred because it requires an
                            // owned value
                            self.emitter.emit(ActionSubmit);
                        }
                        // The group should already be open because it had to be
                        // selected before it was submitted
                        MenuItemDisplay::Group { .. } => self.next_layer(),
                    }
                }
                SelectEvent::Toggle(_) => {}
            });
            layer += 1;
        }

        propagated
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        // Reverse the list so lowest children get priority. It shouldn't
        // actually matter though because only the active menu is focused so
        // only that one gets key events, and there's no visual overlap so they
        // shouldn't be competing for mouse events

        self.stack
            .iter_mut()
            .rev()
            .map(ToChild::to_child_mut)
            .collect()
    }
}

impl Draw for ActionMenuContent {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        // Accumulate x/y offset across all layers
        let mut offset_x = 0;
        let mut offset_y = 0;

        for (i, layer) in self.stack.iter().enumerate() {
            let width = self.layer_width(layer);
            let area = Rect {
                width,
                height: layer.len() as u16,
                x: metadata.area().x + offset_x,
                y: metadata.area().y + offset_y,
            };

            // Add to the offsets for a potential child
            offset_x += width + 1;
            offset_y += layer.selected_index().unwrap_or(0) as u16;

            canvas.draw(
                layer,
                SelectListProps {
                    scrollbar_margin: 0,
                },
                area,
                i == self.active_layer,
            );
        }
    }
}

/// Emitted event to tell the parent when the user has submitted an action
#[derive(Debug)]
struct ActionSubmit;

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
/// [Select]s
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

impl Display for MenuItemDisplay {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self {
            // If a shortcut is given, include the binding in the text
            Self::Action {
                name,
                shortcut: Some(shortcut),
                ..
            } => {
                let s =
                    TuiContext::get().input_engine.add_hint(name, *shortcut);
                write!(fmt, "{s}")
            }
            Self::Action { name, .. } => {
                write!(fmt, "{name}")
            }
            Self::Group { name, .. } => {
                write!(fmt, "{name} ▶")
            }
        }
    }
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
        match &self {
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
fn build_select(items: Vec<MenuItemDisplay>) -> Select<MenuItemDisplay> {
    let disabled_indexes = items
        .iter()
        .enumerate()
        .filter(|(_, item)| {
            matches!(item, MenuItemDisplay::Action { enabled, .. } if !enabled)
        })
        .map(|(i, _)| i)
        .collect_vec();

    Select::builder(items)
        .disabled_indexes(disabled_indexes)
        .subscribe([SelectEventType::Select, SelectEventType::Submit])
        .build()
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
    #[derive(derive_more::Debug)]
    struct Actionable {
        id: ComponentId,
        emitter: Emitter<TestAction>,
        /// List of returned actions is customizable for different test cases
        #[debug(skip)]
        get_actions: Box<dyn Fn(Emitter<TestAction>) -> Vec<MenuItem>>,
    }

    impl Actionable {
        fn new(
            get_actions: impl 'static + Fn(Emitter<TestAction>) -> Vec<MenuItem>,
        ) -> Self {
            Self {
                id: ComponentId::default(),
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

    impl Component for Actionable {
        fn id(&self) -> ComponentId {
            self.id
        }

        fn menu(&self) -> Vec<MenuItem> {
            (self.get_actions)(self.to_emitter())
        }
    }

    impl Draw for Actionable {
        fn draw(&self, _: &mut Canvas, (): (), _: DrawMetadata) {}
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
            .action(&["Action 2"])
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
